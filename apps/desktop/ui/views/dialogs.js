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
export async function openSafetyDialog(userId) {
  const number = await actions.safetyNumber(userId);
  if (!number) return;
  const groups = number.split(' ');
  const copyBtn = el('button', {
    class: 'ghost',
    icon: 'copy',
    text: 'Copy',
    onclick: async () => {
      if (await copyText(number)) toast('Safety number copied', 'info');
    },
  });
  openDialog({
    title: 'Verify safety number',
    content: el('div', {},
      el('p', { class: 'muted', text: 'Compare these numbers with the other person in person or on a call. If they match on both screens, your chat is verified.' }),
      el('ol', { class: 'safety-grid', 'aria-label': 'Safety number' }, groups.map((g) => el('li', { text: g }))),
      el('div', { class: 'row end' }, copyBtn)),
  });
}
