// Main pane: header, message list and composer. The same markup serves channels and DMs.
import { state, subscribe, canModerate, inServer, handleOf } from '../state.js';
import * as actions from '../actions.js';
import { el } from '../ui.js';
import { confirmDialog } from './dialogs.js';
import { openProfile } from './profile.js';

const header = el('header', { class: 'chat-header' });
const list = el('div', { class: 'messages' });
const composerHost = el('div', { class: 'composer-host' });

let composerKey = null;
let listKey = null;
let editingId = null;
let editInput = null; // kept across refreshes so an unsent draft survives

export function initChat() {
  document.getElementById('main').replaceChildren(header, list, composerHost);
  subscribe('main', render);
  render();
}

// Normalises the open channel or DM into one shape the renderer understands.
function current() {
  const me = state.me;
  if (inServer()) {
    const channel = state.community.channels.find((c) => c.id === state.channelId);
    if (!channel) return null;
    return {
      key: `ch:${state.community.id}:${channel.id}`,
      dm: false,
      title: channel.name,
      hash: true,
      subtitle: state.community.name,
      placeholder: `Message #${channel.name}`,
      items: state.messages.map((m) => ({
        id: m.id,
        author: m.author,
        authorName: m.author_name,
        text: m.text,
        ms: m.ms,
        edited: m.edited,
        deleted: m.deleted,
        own: m.author === me?.root_id,
        pending: false,
      })),
    };
  }
  if (state.route.kind === 'home' && state.peer) {
    const name = peerName();
    return {
      key: `dm:${state.peer}`,
      dm: true,
      title: name,
      subtitle: `·${state.peer.slice(0, 8)}`,
      placeholder: `Message ${name}`,
      items: state.dmMessages.map((m) => {
        const own = m.from === me?.root_id;
        return {
          id: m.id,
          author: m.from,
          authorName: own ? me?.display_name : name,
          text: m.text,
          ms: m.ms,
          edited: false,
          deleted: false,
          own,
          pending: m.pending,
        };
      }),
    };
  }
  return null;
}

function peerName() {
  const known = state.conversations.find((c) => c.peer === state.peer)?.display_name;
  return known || state.peerName || `anon·${state.peer.slice(0, 8)}`;
}

function emptyText() {
  if (state.route.kind === 'server') {
    return state.community ? 'Pick a channel to start reading.' : 'Loading server…';
  }
  return 'Pick a conversation, or add a contact to start one.';
}

function render() {
  const view = current();
  if (!view) {
    header.replaceChildren();
    list.replaceChildren(el('p', { class: 'empty', text: emptyText() }));
    composerHost.replaceChildren();
    composerKey = null;
    listKey = null;
    return;
  }
  header.replaceChildren(
    el('h2', { icon: view.hash ? 'hash' : null, text: view.title }),
    el('span', { class: 'muted', text: view.subtitle }),
  );
  // Measure before replacing rows: follow new messages only if the reader was already at the bottom.
  const wasAtBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 80;
  const switched = view.key !== listKey;
  list.replaceChildren(...(view.items.length
    ? view.items.map((item) => messageRow(item, view))
    : [el('p', { class: 'empty', text: 'No messages yet. Say hello.' })]));
  if (switched || wasAtBottom) list.scrollTop = list.scrollHeight;
  listKey = view.key;
  // Rebuild the composer only when the conversation changes, so a half-typed draft survives refreshes.
  if (view.key !== composerKey) {
    composerHost.replaceChildren(composer(view));
    composerKey = view.key;
  }
}

function messageRow(item, view) {
  const editing = editingId === item.id;
  const meta = el('div', { class: 'meta' },
    el('button', {
      class: 'author',
      text: item.authorName || 'unknown',
      onclick: (e) => openProfile(item.author, e.clientX, e.clientY),
    }),
    handleOf(item.author) ? el('span', { class: 'handle', text: `@${handleOf(item.author)}` }) : null,
    el('time', { text: formatTime(item.ms) }),
    item.edited && !item.deleted ? el('span', { class: 'edited', text: '(edited)' }) : null,
  );
  let body;
  if (editing) body = editForm(item);
  else if (item.deleted) body = el('p', { class: 'body deleted', text: 'message deleted' });
  else body = el('p', { class: 'body', text: item.text });
  return el('article', { class: `msg${item.pending ? ' pending' : ''}`, 'data-id': item.id },
    meta,
    body,
    item.pending ? el('span', { class: 'pending-note', icon: 'clock', text: 'sending…' }) : null,
    view.dm || editing ? null : rowActions(item, view),
  );
}

function rowActions(item) {
  if (item.deleted) return null;
  const buttons = [];
  if (item.own) {
    buttons.push(el('button', {
      icon: 'pencil',
      text: 'Edit',
      onclick: () => {
        editingId = item.id;
        editInput = null;
        render();
      },
    }));
  }
  if (item.own || canModerate()) {
    buttons.push(el('button', { class: 'danger-text', icon: 'trash-2', text: 'Delete', onclick: () => removeMessage(item.id) }));
  }
  return buttons.length ? el('div', { class: 'msg-actions' }, buttons) : null;
}

async function removeMessage(messageId) {
  if (!(await confirmDialog('Delete this message?', 'Delete'))) return;
  await actions.deleteMessage(messageId);
}

function editForm(item) {
  if (!editInput) {
    editInput = el('textarea', { class: 'edit-input', rows: 2, value: item.text, 'aria-label': 'Edit message' });
    editInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        save(editingId);
      } else if (e.key === 'Escape') {
        cancel();
      }
    });
    setTimeout(() => editInput.focus(), 0);
  }
  const input = editInput;
  const cancel = () => {
    editingId = null;
    editInput = null;
    render();
  };
  const save = async (id) => {
    const text = input.value.trim();
    if (!text || id === null) return;
    if (await actions.editMessage(id, text)) {
      editingId = null;
      editInput = null;
      render();
    }
  };
  return el('div', { class: 'edit' },
    input,
    el('div', { class: 'row' },
      el('button', { class: 'primary', text: 'Save', onclick: () => save(editingId) }),
      el('button', { class: 'ghost', text: 'Cancel', onclick: cancel })),
  );
}

function composer(view) {
  const input = el('textarea', { rows: 1, placeholder: view.placeholder, 'aria-label': 'Message' });
  const send = async () => {
    const text = input.value.trim();
    if (!text) return;
    input.disabled = true;
    const ok = view.dm ? await actions.sendDm(text) : await actions.sendMessage(text);
    input.disabled = false;
    if (ok) input.value = '';
    input.focus();
  };
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  });
  return el('div', { class: 'composer' }, input, el('button', { class: 'primary', icon: 'send-horizontal', text: 'Send', onclick: send }));
}

function formatTime(ms) {
  return new Date(Number(ms)).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
}
