// Settings > Recovery (recovery guardians) and Settings > Devices. Both need their feature; Settings only lists them then.
import { api } from '../api.js';
import { el, icon, avatar, copyText, toast } from '../ui.js';

const MAX_GUARDIANS = 16;

// The guardian picker only offers contacts; the threshold is 1 to the number of guardians chosen.
export async function recoveryNodes({ onCancel } = {}) {
  let view;
  try {
    view = await api.listGuardians();
  } catch (err) {
    return [el('p', { class: 'error', text: `Could not load guardians: ${err?.message ?? err}` })];
  }
  let guardians = view.guardians.map((g) => ({ ...g }));
  let threshold = view.threshold;
  // The last saved state. Cancel drops unsaved edits back to it.
  let saved = { guardians: guardians.map((g) => ({ ...g })), threshold };
  const save = el('button', { class: 'primary', text: 'Sign and save' });
  const cancel = el('button', { class: 'ghost', text: 'Cancel', onclick: () => {
    guardians = saved.guardians.map((g) => ({ ...g }));
    threshold = saved.threshold;
    paint();
    onCancel?.();
  } });
  const heading = el('h3', { class: 'block-title' });
  const list = el('ul', { class: 'guardian-list', 'aria-label': 'Guardians' });
  const picker = el('div', { class: 'guardian-picker', hidden: true });
  const addBtn = el('button', { class: 'ghost', icon: 'user-plus', text: 'Add guardian' });
  addBtn.addEventListener('click', async () => {
    picker.hidden = !picker.hidden;
    if (!picker.hidden) await paintPicker();
  });
  const thresholds = el('div', { class: 'threshold-row', role: 'radiogroup', 'aria-label': 'How many must agree' });
  const hint = el('p', { class: 'muted' });

  const paint = () => {
    list.replaceChildren(...guardians.map((g) => el('li', { class: 'guardian-row' },
      avatar(g.display_name, g.root_id),
      el('span', { class: 'text' },
        el('span', { class: 'name', text: g.display_name }),
        el('span', { class: 'tag', text: `·${g.suffix}` })),
      el('span', { class: 'shared', text: g.shared_servers ? `${g.shared_servers} shared server${g.shared_servers === 1 ? '' : 's'}` : 'Contact' }),
      el('button', {
        class: 'icon-btn',
        'aria-label': `Remove ${g.display_name}`,
        icon: 'x',
        onclick: () => {
          guardians = guardians.filter((x) => x.root_id !== g.root_id);
          threshold = Math.min(threshold, Math.max(1, guardians.length));
          paint();
        },
      }))));
    thresholds.replaceChildren(...Array.from({ length: Math.max(1, guardians.length) }, (_, i) => i + 1).map((n) => el('button', {
      class: `threshold${n === threshold ? ' on' : ''}`,
      role: 'radio',
      'aria-checked': n === threshold ? 'true' : 'false',
      text: `${n} of ${Math.max(1, guardians.length)}`,
      onclick: () => {
        threshold = n;
        paint();
      },
    })));
    const words = { 1: 'Any one guardian can move your identity. Fast, but a single compromised friend is enough.' };
    const WORD = ['Zero', 'One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight', 'Nine', 'Ten', 'Eleven', 'Twelve', 'Thirteen', 'Fourteen', 'Fifteen', 'Sixteen'];
    hint.textContent = words[threshold] ?? `${WORD[threshold]} of your ${WORD[guardians.length].toLowerCase()} guardians must approve. One lost or unreachable guardian won’t lock you out.`;
    heading.textContent = `Guardians — ${guardians.length} of ${MAX_GUARDIANS}`;
    save.disabled = guardians.length === 0 || threshold < 1 || threshold > guardians.length;
  };

  // Contacts that are not guardians yet. Picking one adds it, up to the maximum.
  const paintPicker = async () => {
    let contacts;
    try {
      contacts = (await api.listContacts()).contacts;
    } catch (err) {
      picker.replaceChildren(el('p', { class: 'error', text: `Could not load contacts: ${err?.message ?? err}` }));
      return;
    }
    const chosen = new Set(guardians.map((g) => g.root_id));
    const free = contacts.filter((c) => !chosen.has(c.root_id));
    picker.replaceChildren(...(free.length ? free.map((c) => el('button', {
      class: 'ghost guardian-pick',
      text: c.display_name,
      onclick: () => {
        if (guardians.length >= MAX_GUARDIANS) {
          toast(`You can have up to ${MAX_GUARDIANS} guardians`);
          return;
        }
        guardians = [...guardians, { root_id: c.root_id, display_name: c.display_name, suffix: c.suffix, shared_servers: 0 }];
        picker.hidden = true;
        paint();
      },
    })) : [el('p', { class: 'muted', text: 'Every contact is already a guardian.' })]));
  };

  save.onclick = async () => {
    try {
      const signed = await api.signGuardians(guardians.map((g) => g.root_id), threshold);
      guardians = signed.guardians.map((g) => ({ ...g }));
      threshold = signed.threshold;
      saved = { guardians: guardians.map((g) => ({ ...g })), threshold };
      toast('Guardians saved', 'info');
      paint();
    } catch (err) {
      toast(err);
    }
  };

  paint();
  return [
    el('section', { class: 'recovery' },
      el('h2', { class: 'section-title', text: 'Recovery' }),
      el('p', { class: 'muted', text: 'If you lose your recovery phrase, enough of your guardians can vouch for a new key. Your roles and handles in every server move with you. Guardians never hold a copy of your key.' }),
      el('div', { class: 'guardian-head' }, heading, addBtn),
      picker,
      list,
      el('h3', { class: 'block-title', text: 'How many must agree' }),
      thresholds,
      hint,
      el('div', { class: 'callout' }, icon('key-round'), el('span', { text: 'Saving signs this list with your key and shares it with your devices. To recover, open Recover account on a new device and send the link it shows to your guardians.' })),
      el('div', { class: 'settings-footer' }, cancel, save)),
  ];
}

// Linked devices: one row per device, with revoke for the others and a code to link a new one.
export async function devicesNodes() {
  let devices;
  try {
    devices = await api.listDevices();
  } catch (err) {
    return [el('p', { class: 'error', text: `Could not load devices: ${err?.message ?? err}` })];
  }
  const rows = devices.map((d) => el('li', { class: 'device-row' },
    el('span', { class: 'text' },
      el('span', { class: 'name', text: d.name }),
      el('span', { class: 'muted', text: d.this_device ? 'This device' : `Last seen ${new Date(Number(d.last_seen_ms)).toLocaleString('en-US')}` })),
    d.this_device ? null : el('button', {
      class: 'danger-outline',
      text: 'Remove',
      onclick: async () => {
        try {
          await api.revokeDevice(d.device_id);
          toast('Device removed', 'info');
        } catch (err) {
          toast(err);
        }
      },
    })));
  const linkBtn = el('button', {
    class: 'ghost',
    text: 'Link a new device',
    onclick: async () => {
      try {
        const code = await api.linkDeviceCode();
        if (await copyText(code)) toast('Link code copied', 'info');
      } catch (err) {
        toast(err);
      }
    },
  });
  return [
    el('section', {},
      el('h2', { class: 'section-title', text: 'Devices' }),
      el('p', { class: 'muted', text: 'Devices that hold your identity. Remove one to revoke its access.' }),
      el('ul', { class: 'device-list', 'aria-label': 'Devices' }, rows),
      el('div', { class: 'row end' }, linkBtn),
      linkingNodes()),
  ];
}

// Device linking (Part 6.6 and 6.15). Root device: paste the new device's code, approve, and copy the grant back.
// New device: paste the grant and finish, which stores the identity and restarts the engine.
function linkingNodes() {
  const codeIn = el('input', { placeholder: 'dissent://link/…', 'aria-label': 'Link code from the new device' });
  const grantOut = el('input', { readOnly: true, placeholder: 'The grant appears here', 'aria-label': 'Grant for the new device' });
  const grantCopy = el('button', { class: 'ghost', text: 'Copy grant', onclick: async () => { if (grantOut.value && (await copyText(grantOut.value))) toast('Grant copied', 'info'); } });
  const approve = el('button', { class: 'primary', text: 'Approve', onclick: async () => {
    const link = codeIn.value.trim();
    if (!link) return;
    try {
      grantOut.value = await api.approveDevice(link);
      toast('New device approved. Send it the grant.', 'info');
    } catch (err) {
      toast(err);
    }
  } });
  const grantIn = el('input', { placeholder: 'Paste the grant from the other device', 'aria-label': 'Grant from the other device' });
  const finish = el('button', { class: 'primary', text: 'Finish linking', onclick: async () => {
    const grant = grantIn.value.trim();
    if (!grant) return;
    try {
      await api.finishDeviceLink(grant);
      toast('This device is linked. Dissent is restarting.', 'info');
    } catch (err) {
      toast(err);
    }
  } });
  return el('div', { class: 'linking' },
    el('h3', { class: 'block-title', text: 'Approve a new device' }),
    el('p', { class: 'muted', text: 'Paste the link code shown on the new device, then send back the grant it produces.' }),
    el('div', { class: 'row' }, codeIn, approve),
    el('div', { class: 'row' }, grantOut, grantCopy),
    el('h3', { class: 'block-title', text: 'Link this device' }),
    el('p', { class: 'muted', text: 'Paste the grant from a device that already has your identity.' }),
    el('div', { class: 'row' }, grantIn, finish));
}
