// Modal dialogs: create/join server, invite links, contact link, add contact, create channel, settings, confirm.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { el, icon, avatar, openDialog, copyText, toast } from '../ui.js';
import { has } from '../features.js';

// Resolves true on confirm, false on cancel or dismiss.
export function confirmDialog(message, okLabel) {
  return new Promise((resolve) => {
    let close = () => {};
    const ok = el('button', {
      class: 'danger',
      text: okLabel,
      onclick: () => {
        resolve(true);
        close();
      },
    });
    const cancel = el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() });
    ({ close } = openDialog({
      title: 'Please confirm',
      content: el('div', {}, el('p', { text: message }), el('div', { class: 'row end' }, cancel, ok)),
      onClose: () => resolve(false),
    }));
  });
}

// Create a server, or join one with an invite link. Each button runs its action and closes on success.
export function openServerDialog() {
  let close = () => {};
  const tabs = {};
  const panels = {};
  const select = (name) => {
    for (const key of Object.keys(tabs)) {
      tabs[key].classList.toggle('active', key === name);
      panels[key].hidden = key !== name;
    }
  };
  const nameInput = el('input', { placeholder: 'Server name', maxLength: 64, 'aria-label': 'Server name' });
  const createBtn = el('button', {
    class: 'primary',
    text: 'Create server',
    onclick: async () => {
      const name = nameInput.value.trim();
      if (!name) return;
      createBtn.disabled = true;
      const summary = await actions.createCommunity(name);
      createBtn.disabled = false;
      if (summary) close();
    },
  });
  const inviteInput = el('textarea', { rows: 3, placeholder: 'dissent://invite/…', 'aria-label': 'Invite link' });
  const joinBtn = el('button', {
    class: 'primary',
    text: 'Join server',
    onclick: async () => {
      const invite = inviteInput.value.trim();
      if (!invite) return;
      joinBtn.disabled = true;
      const summary = await actions.joinCommunity(invite);
      joinBtn.disabled = false;
      if (summary) close();
    },
  });
  tabs.create = el('button', { class: 'tab active', text: 'Create server', onclick: () => select('create') });
  tabs.join = el('button', { class: 'tab', text: 'Join server', onclick: () => select('join') });
  panels.create = el('div', { class: 'panel' }, el('p', { class: 'muted', text: 'You become the owner. A "general" channel is created for you.' }), nameInput, el('div', { class: 'row end' }, createBtn));
  panels.join = el('div', { class: 'panel', hidden: true }, el('p', { class: 'muted', text: 'Paste an invite link from someone in the server.' }), inviteInput, el('div', { class: 'row end' }, joinBtn));
  ({ close } = openDialog({
    title: 'Servers',
    content: el('div', {}, el('div', { class: 'tabs' }, tabs.create, tabs.join), panels.create, panels.join),
  }));
}

// Shows a link that is loaded on open, with a Copy button.
function openLinkDialog(title, description, loadLink) {
  const input = el('input', { readOnly: true, class: 'link-input', placeholder: 'Creating link…', 'aria-label': title });
  const copy = el('button', {
    class: 'primary',
    icon: 'copy',
    text: 'Copy',
    disabled: true,
    onclick: async () => {
      if (await copyText(input.value)) {
        toast('Copied to clipboard', 'info');
        flashCopied(copy);
      }
    },
  });
  openDialog({
    title,
    content: el('div', {}, el('p', { class: 'muted', text: description }), el('div', { class: 'row' }, input, copy)),
  });
  loadLink().then((link) => {
    if (!link) {
      input.placeholder = 'Could not create link';
      return;
    }
    input.value = link;
    copy.disabled = false;
  });
}

export function openInviteDialog(communityId) {
  openLinkDialog(
    'Invite people',
    'Anyone with this link can join the server. Share it out of band.',
    () => actions.createInvite(communityId),
  );
}

export function openContactDialog() {
  openLinkDialog(
    'My contact link',
    'Share this so someone can message you directly, even without a shared server.',
    () => actions.contactLink(),
  );
}

export function openAddContactDialog() {
  let close = () => {};
  const input = el('textarea', { rows: 3, placeholder: 'dissent://contact/…', 'aria-label': 'Contact link' });
  const addBtn = el('button', {
    class: 'primary',
    icon: 'user-plus',
    text: 'Add contact',
    onclick: async () => {
      const link = input.value.trim();
      if (!link) return;
      addBtn.disabled = true;
      const ok = await actions.addContact(link);
      addBtn.disabled = false;
      if (ok) close();
    },
  });
  ({ close } = openDialog({
    title: 'Add contact',
    content: el('div', {}, el('p', { class: 'muted', text: 'Paste a contact link to start a direct conversation.' }), input, el('div', { class: 'row end' }, addBtn)),
  }));
}

export function openCreateChannelDialog(communityId) {
  let close = () => {};
  const input = el('input', { placeholder: 'channel-name', maxLength: 64, 'aria-label': 'Channel name' });
  const createBtn = el('button', {
    class: 'primary',
    icon: 'plus',
    text: 'Create channel',
    onclick: async () => {
      const name = input.value.trim();
      if (!name) return;
      createBtn.disabled = true;
      const ok = await actions.createChannel(communityId, name);
      createBtn.disabled = false;
      if (ok) close();
    },
  });
  ({ close } = openDialog({
    title: 'Create channel',
    content: el('div', {}, input, el('div', { class: 'row end' }, createBtn)),
  }));
}

export function openClaimHandleDialog(communityId) {
  let close = () => {};
  const input = el('input', { placeholder: 'your-handle', maxLength: 32, 'aria-label': 'Handle' });
  const claimBtn = el('button', {
    class: 'primary',
    icon: 'at-sign',
    text: 'Claim handle',
    onclick: async () => {
      const handle = input.value.trim();
      if (!handle) return;
      claimBtn.disabled = true;
      const ok = await actions.claimHandle(communityId, handle);
      claimBtn.disabled = false;
      if (ok) close();
    },
  });
  ({ close } = openDialog({
    title: 'Claim handle',
    content: el('div', {},
      el('p', { class: 'muted', text: 'Your handle is shown next to your name in this server.' }),
      input,
      el('div', { class: 'row end' }, claimBtn)),
  }));
}

// Swaps a Copy button to a check mark for a moment, so the copy is visibly confirmed.
function flashCopied(btn) {
  btn.replaceChildren(icon('check'), 'Copied');
  setTimeout(() => btn.replaceChildren(icon('copy'), 'Copy'), 1500);
}

// Safety number: the same 12 groups of five digits on both sides. Compare it in person or on a call.
// Safety number. With `key_change` on and the peer's key changed (Part 6.10), an amber banner comes first and
// "They match" marks the number compared (`mark_safety_verified`). Without `safety`, the button is not shown.
export async function openSafetyDialog(userId) {
  const number = await actions.safetyNumber(userId);
  if (!number) return;
  const groups = number.split(' ');
  const profile = state.peer === userId ? state.peerProfile : null;
  const name = profile?.display_name ?? 'This person';
  const changed = has('key_change') && Boolean(profile?.key_changed_ms);
  let close = () => {};
  const notNowBtn = el('button', { class: 'ghost', text: 'Not now', onclick: () => close() });
  const matchBtn = has('safety')
    ? el('button', {
      class: 'primary',
      text: 'They match',
      onclick: async () => {
        if (await actions.markSafetyVerified(userId)) {
          toast('Safety number marked as compared', 'info');
          close();
        }
      },
    })
    : null;
  ({ close } = openDialog({
    title: changed ? `Safety number with ${name}` : 'Verify safety number',
    content: el('div', {},
      changed
        ? el('div', { class: 'key-banner', role: 'status' }, icon('circle-alert'),
          el('span', { text: `${name}'s key changed on ${new Date(Number(profile.key_changed_ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}. They may have recovered their account. Compare before you trust new messages.` }))
        : null,
      el('p', { class: 'muted', text: changed
        ? `Compare these numbers with ${name} in person or on a call. If they match on both screens, nobody has swapped in a different key for either of you.`
        : `Compare these numbers with ${name} in person or on a call. If they match on both screens, your chat is verified.` }),
      el('ol', { class: 'safety-grid', 'aria-label': 'Safety number' }, groups.map((g) => el('li', { text: g }))),
      el('div', { class: 'row end' }, notNowBtn, matchBtn)),
  }));
}

// Create a group (NewGroup board): pick at least three contacts, see them as chips, name the group, then create it.
// Needs the groups feature. The footer button says how many people the group will have, counting me.
export function openNewGroupDialog() {
  const people = state.contacts?.contacts ?? [];
  const picked = new Set();
  const nameInput = el('input', { value: 'New group', 'aria-label': 'Group name' });
  const search = el('input', { placeholder: 'Type a name or paste a contact link', 'aria-label': 'Find contacts' });
  const count = el('span', { class: 'pick-count' });
  const chips = el('div', { class: 'pick-chips' });
  const list = el('ul', { class: 'pick-list', 'aria-label': 'Contacts' });
  const createBtn = el('button', { class: 'primary', text: 'Create group' });
  const update = () => {
    const n = picked.size;
    const ok = n >= 3;
    count.textContent = ok ? `${n} selected · group of ${n + 1} with you` : `Pick ${3 - n} more`;
    count.classList.toggle('short', !ok);
    createBtn.disabled = !ok;
    createBtn.textContent = ok ? `Create group of ${n + 1}` : 'Create group';
    chips.replaceChildren(...people.filter((p) => picked.has(p.root_id)).map((p) => el('button', {
      class: 'pick-chip',
      'aria-label': `Remove ${p.display_name}`,
      onclick: () => {
        picked.delete(p.root_id);
        paint();
        update();
      },
    }, avatar(p.display_name, p.root_id, 'sm'), el('span', { text: p.display_name }), icon('x'))));
  };
  const paint = () => {
    const q = search.value.trim().toLowerCase();
    const shown = people.filter((p) => !q || p.display_name.toLowerCase().includes(q) || p.suffix.includes(q));
    list.replaceChildren(...shown.map((p) => el('li', {},
      el('label', { class: 'pick-row' + (picked.has(p.root_id) ? ' on' : '') },
        avatar(p.display_name, p.root_id),
        el('span', { class: 'pick-text' },
          el('span', { class: 'pick-name', text: p.display_name }),
          el('span', { class: 'pick-tag', text: '·' + p.suffix })),
        el('input', {
          type: 'checkbox',
          checked: picked.has(p.root_id),
          onchange: (e) => {
            if (e.target.checked) picked.add(p.root_id);
            else picked.delete(p.root_id);
            paint();
            update();
          },
        })))));
    if (!shown.length) list.replaceChildren(el('li', { class: 'pick-empty muted', text: people.length ? 'No contacts match.' : 'Add a contact first.' }));
    update();
  };
  search.addEventListener('input', paint);
  let close = () => {};
  ({ close } = openDialog({
    title: 'Create a group',
    content: el('div', { class: 'new-group' },
      el('p', { class: 'muted', text: 'Pick at least 3 people. Only the people in the group can read it.' }),
      el('div', { class: 'pick-field' }, chips, search),
      el('div', { class: 'pick-head' }, el('span', { class: 'section-label', text: 'Contacts' }), count),
      list,
      el('label', { class: 'field-label' }, el('span', { class: 'section-label', text: 'Group name' }), nameInput),
      el('div', { class: 'row end' }, el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() }), createBtn)),
  }));
  createBtn.onclick = async () => {
    const ok = await actions.createGroup(nameInput.value.trim() || 'New group', [...picked]);
    if (ok) close();
  };
  paint();
  search.focus();
}

// Private note or nickname on a contact (notes feature): one dialog, two kinds.
export function openNoteDialog(peer, name, kind) {
  const isNote = kind === 'note';
  const input = el('textarea', { rows: 3, 'aria-label': isNote ? 'Note' : 'Nickname' });
  const saveBtn = el('button', { class: 'primary', text: 'Save' });
  const { close } = openDialog({
    title: isNote ? `Note on ${name}` : `Nickname for ${name}`,
    content: el('div', {},
      el('p', { class: 'muted', text: isNote ? 'Only visible to you.' : 'Shown in place of the name, only on this device.' }),
      input,
      el('div', { class: 'row end' }, el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() }), saveBtn)),
  });
  saveBtn.onclick = async () => {
    if (await actions.setNote(peer, kind, input.value.trim())) close();
  };
}

// Share a file (FileSend board). Opens after the native picker when the file-sharing UI is on; the target is the open
// channel or the open DM. The copy is honest: files stay with the people who receive them, and the limit is 10 GB.
const MAX_SHARE_BYTES = 10 * 1024 ** 3;

export function openFileSendDialog(files) {
  const targets = [];
  if (state.community && state.channelId) {
    const channel = state.community.channels.find((c) => c.id === state.channelId);
    if (channel) {
      targets.push({
        key: 'channel',
        title: '#' + channel.name,
        sub: state.community.name + ' · ' + state.community.members.filter((m) => !m.banned).length + ' members',
        initials: state.community.name.split(/\s+/).map((w) => w[0]).join('').slice(0, 2).toUpperCase(),
        target: { kind: 'channel', communityId: state.community.id, channelId: channel.id, seed: true },
        steps: [
          'Your device locks the file with a new key and keeps the encrypted copy.',
          'The key goes out in a sealed message only the ' + state.community.members.filter((m) => !m.banned).length + ' members of #' + channel.name + ' can open.',
          'Members download from you and from each other at the same time, and from the seeder when you’re offline.',
        ],
        via: 'Members share the load',
        members: state.community.members.filter((m) => !m.banned).length,
        end: 'Members',
        send: 'Share to #' + channel.name,
      });
    }
  }
  // A direct target: one member, peer to peer (FileSend board, "Maya, directly").
  const directTarget = (name, peer) => ({
    key: 'direct',
    title: name + ', directly',
    sub: 'Peer to peer · just the two of you',
    initials: name.slice(0, 2).toUpperCase(),
    target: { kind: 'dm', peer },
    steps: [
      'Your device locks the file with a new key. Only ' + name + ' gets the key.',
      'It goes straight from your device to theirs. If your networks block that, a relay passes along the encrypted bytes.',
      'If ' + name + ' is offline, it waits on your device. Keep Dissent running until it’s delivered.',
    ],
    via: 'Direct, or a relay if needed',
    end: name,
    send: 'Send to ' + name,
  });
  if (state.route.kind === 'home' && state.peer && !state.group) {
    targets.push(directTarget(state.peerName || 'them', state.peer));
  } else if (state.community && state.channelId) {
    const other = state.community.members.find((m) => !m.banned && m.root_id !== state.me?.root_id);
    if (other) targets.push(directTarget(other.display_name, other.root_id));
  }
  if (targets.length === 0) return;
  let current = targets[0];
  const total = files.reduce((n, f) => n + (Number(f.size) || 0), 0);
  const over = files.some((f) => Number(f.size) > MAX_SHARE_BYTES);
  const note = el('input', { placeholder: 'Add a message (optional)', 'aria-label': 'Add a message' });
  const seedBox = el('input', { type: 'checkbox', checked: true, 'aria-label': 'Let the Shop seeder keep an encrypted copy' });
  const seedRow = el('label', { class: 'seed-row' }, seedBox, el('span', { class: 'seed-text' },
    el('span', { class: 'seed-title', text: 'Let the seeder keep an encrypted copy' }),
    el('span', { class: 'seed-sub', text: 'Members can still download it when you’re offline. The seeder never gets the key, so it can’t open the file.' })));
  const sendBtn = el('button', { class: 'primary', text: current.send, disabled: over });
  const travel = el('div', { class: 'travel' });
  const picks = el('div', { class: 'targets', role: 'radiogroup', 'aria-label': 'Send to' });
  const paint = () => {
    picks.replaceChildren(...targets.map((t) => el('button', {
      class: 'target' + (t === current ? ' on' : ''),
      role: 'radio',
      'aria-checked': t === current ? 'true' : 'false',
      onclick: () => {
        current = t;
        paint();
      },
    }, el('span', { class: 'target-mark', text: t.initials }), el('span', { class: 'target-text' }, el('span', { class: 'target-title', text: t.title }), el('span', { class: 'target-sub', text: t.sub })))));
    // The channel end counts its members, and names the seeder when the seeder keeps a copy.
    const end = current.members === undefined ? current.end : `${current.members} members${seedBox.checked ? ' + seeder' : ''}`;
    travel.replaceChildren(
      el('span', { class: 'travel-label', text: 'How it travels' }),
      el('div', { class: 'travel-line' }, el('span', { class: 'end-node', text: 'You' }), el('span', { class: 'line' }), el('span', { class: 'via', text: current.via }), el('span', { class: 'line' }), el('span', { class: 'end-node', text: end })),
      el('ol', { class: 'steps' }, current.steps.map((s, i) => el('li', {}, el('span', { class: 'n', text: String(i + 1) }), el('span', { text: s })))));
    sendBtn.textContent = current.send;
    seedRow.hidden = current.target.kind !== 'channel';
  };
  seedBox.addEventListener('change', paint);
  paint();
  const fileRow = el('div', { class: 'file-card' },
    el('span', { class: 'file-tile', text: files.length === 1 ? (fileExt(files[0].name) || 'FILE').toUpperCase() : String(files.length) }),
    el('span', { class: 'file-text' },
      el('span', { class: 'file-name', text: files.length === 1 ? files[0].name : files.length + ' files' }),
      el('span', { class: 'file-size', text: formatShareSize(total) + (files.length === 1 ? '' : ' in total') })),
    el('span', { class: 'limit-pill' + (over ? ' over' : ''), text: over ? 'Over the 10 GB limit' : 'Under the 10 GB limit' }));
  let close = () => {};
  ({ close } = openDialog({
    title: 'Share a file',
    content: el('div', { class: 'file-send' },
      fileRow,
      el('span', { class: 'section-label', text: 'Send to' }),
      picks,
      travel,
      seedRow,
      el('div', { class: 'warn-note' }, icon('circle-alert'), el('span', { text: 'Anyone who receives this file keeps access to it, even if they’re removed later.' })),
      note,
      el('div', { class: 'row end' }, el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() }), sendBtn)),
  }));
  sendBtn.onclick = async () => {
    sendBtn.disabled = true;
    const target = current.target.kind === 'channel' ? { ...current.target, seed: seedBox.checked } : current.target;
    const ok = await actions.sendFiles(files.map((f) => f.path), note.value.trim() || null, target);
    if (ok) close();
    else sendBtn.disabled = false;
  };
}

function fileExt(name) {
  const n = String(name);
  return n.includes('.') ? n.split('.').pop() : '';
}

function formatShareSize(n) {
  const b = Number(n) || 0;
  if (b >= 1024 ** 3) return (b / 1024 ** 3).toFixed(1) + ' GB';
  if (b >= 1024 ** 2) return (b / 1024 ** 2).toFixed(1) + ' MB';
  if (b >= 1024) return (b / 1024).toFixed(1) + ' KB';
  return b + ' B';
}
