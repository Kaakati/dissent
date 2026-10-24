// Modal dialogs: create/join server, invite links, contact link, add contact, create channel, settings, confirm.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { el, icon, openDialog, copyText, toast } from '../ui.js';

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

export function openSettings() {
  let close = () => {};
  const me = state.me;

  const nameInput = el('input', { value: me?.display_name ?? '', maxLength: 64, 'aria-label': 'Display name' });
  const saveBtn = el('button', {
    class: 'primary',
    text: 'Save',
    onclick: async () => {
      const name = nameInput.value.trim();
      if (!name) {
        toast('Display name cannot be empty');
        return;
      }
      await actions.setDisplayName(name);
    },
  });

  const phraseBox = el('textarea', { readOnly: true, rows: 3, hidden: true, 'aria-label': 'Recovery phrase' });
  const phraseWarning = el('p', {
    class: 'warning',
    hidden: true,
    text: 'Anyone with these 24 words controls your identity. Keep them offline and never share them.',
  });
  const phraseLabel = el('span', { text: 'Show recovery phrase' });
  const phraseBtn = el('button', { class: 'ghost', icon: 'key-round' }, phraseLabel);
  phraseBtn.addEventListener('click', async () => {
    if (!phraseBox.hidden) {
      phraseBox.hidden = true;
      phraseWarning.hidden = true;
      phraseBox.value = '';
      phraseLabel.textContent = 'Show recovery phrase';
      return;
    }
    const phrase = await actions.loadRecoveryPhrase();
    if (!phrase) return;
    phraseBox.value = phrase;
    phraseBox.hidden = false;
    phraseWarning.hidden = false;
    phraseLabel.textContent = 'Hide recovery phrase';
  });

  const importInput = el('textarea', { rows: 3, placeholder: '24 recovery words, separated by spaces', 'aria-label': 'Recovery phrase to import' });
  const importBtn = el('button', {
    class: 'danger-outline',
    icon: 'key-round',
    text: 'Import phrase',
    onclick: async () => {
      const phrase = importInput.value.trim();
      if (!phrase) return;
      const sure = await confirmDialog('Replace the identity on this device with the one from this phrase?', 'Import');
      if (!sure) return;
      if (await actions.restoreIdentity(phrase)) {
        importInput.value = '';
        close();
      }
    },
  });

  const versionLine = el('p', { class: 'muted', text: `Dissent ${state.version ? `v${state.version}` : ''}`.trim() });

  ({ close } = openDialog({
    title: 'Settings',
    content: el('div', { class: 'settings' },
      el('section', {},
        el('h3', { text: 'Display name' }),
        el('div', { class: 'row' }, nameInput, saveBtn)),
      el('section', {},
        el('h3', { text: 'Recovery phrase' }),
        el('div', { class: 'row' }, phraseBtn),
        phraseWarning,
        phraseBox),
      el('section', {},
        el('h3', { text: 'Import recovery phrase' }),
        el('p', { class: 'muted', text: 'Restoring replaces the identity stored on this device.' }),
        importInput,
        el('div', { class: 'row end' }, importBtn)),
      networkSection(),
      el('section', {}, versionLine)),
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

// Relay settings. Public relays are the default. Custom relays apply only when public relays are off.
function networkSection() {
  let settings = { use_public_relays: true, relays: [] };
  const toggle = el('input', { type: 'checkbox', 'aria-label': 'Use public n0 relays' });
  const list = el('ul', { class: 'plain relay-list', 'aria-label': 'Custom relays' });
  const input = el('input', { placeholder: 'https://relay.example.org', 'aria-label': 'Relay URL' });
  const status = el('p', { class: 'muted', role: 'status' });
  const hint = el('p', { class: 'warning', role: 'status' });
  const saveBtn = el('button', { class: 'primary', text: 'Save network settings' });

  // With public relays off and no custom relay, the engine would have no relay at all. Save is disabled instead.
  const render = () => {
    const empty = !settings.use_public_relays && settings.relays.length === 0;
    hint.textContent = empty ? 'Add at least one relay, or turn public relays back on.' : '';
    saveBtn.disabled = empty;
    toggle.checked = settings.use_public_relays;
    list.replaceChildren(...settings.relays.map((url, i) => el('li', { class: 'relay-item' },
      el('span', { class: 'mono', text: url }),
      el('button', {
        class: 'icon-btn',
        'aria-label': `Remove ${url}`,
        title: 'Remove',
        icon: 'x',
        onclick: () => {
          settings.relays = settings.relays.filter((_, j) => j !== i);
          render();
        },
      }))));
    list.hidden = settings.relays.length === 0;
  };

  toggle.addEventListener('change', () => {
    settings.use_public_relays = toggle.checked;
    render();
  });
  const addBtn = el('button', {
    class: 'ghost',
    text: 'Add relay',
    onclick: () => {
      let url;
      try {
        url = new URL(input.value.trim());
      } catch {
        status.textContent = 'Enter a full URL, for example https://relay.example.org';
        return;
      }
      if (url.protocol !== 'https:') {
        status.textContent = 'A relay URL must use https.';
        return;
      }
      if (settings.relays.length >= 8) {
        status.textContent = 'Up to 8 custom relays.';
        return;
      }
      if (!settings.relays.includes(url.toString())) settings.relays = [...settings.relays, url.toString()];
      input.value = '';
      status.textContent = '';
      render();
    },
  });
  saveBtn.addEventListener('click', async () => {
    saveBtn.disabled = true;
    const saved = await actions.setNetworkSettings(settings);
    saveBtn.disabled = false;
    if (saved) {
      settings = saved;
      render();
    }
  });

  actions.loadNetworkSettings().then((loaded) => {
    if (loaded) {
      settings = loaded;
      render();
    }
  });
  render();
  return el('section', {},
    el('h3', { text: 'Network' }),
    el('label', { class: 'toggle-row' }, toggle, el('span', { text: 'Use public n0 relays' })),
    el('p', { class: 'muted', text: 'With public relays on, your relays are added to number0’s. With it off, only your relays are used, and you need at least one. Each must be an https URL.' }),
    list,
    el('div', { class: 'row' }, input, addBtn),
    status,
    hint,
    el('div', { class: 'row end' }, saveBtn));
}
