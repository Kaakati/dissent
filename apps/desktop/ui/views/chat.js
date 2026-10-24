// Main pane: header, message list and composer. The same markup serves channels and DMs.
import { state, subscribe, canModerate, inServer } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, icon, linkify, dayKey, dayLabel, clockLabel, stampLabel, setHue, toast, copyText } from '../ui.js';
import { confirmDialog, openInviteDialog } from './dialogs.js';
import { openProfile } from './profile.js';

const GROUP_MS = 7 * 60 * 1000; // consecutive messages by one author within this gap share a header

// The header and composer keep their DOM across renders, so the search box keeps focus while typing.
const titleBox = el('div', { class: 'chat-title' });
const tools = el('div', { class: 'header-tools' });
const searchInput = el('input', { type: 'search', placeholder: 'Search', 'aria-label': 'Search messages', autocomplete: 'off' });
searchInput.addEventListener('input', () => actions.setFilter(searchInput.value));
const header = el('header', { class: 'chat-header' }, titleBox, tools, el('div', { class: 'search' }, searchInput, icon('search')));
const list = el('div', { class: 'messages', role: 'log', 'aria-label': 'Messages' });
const composerHost = el('div', { class: 'composer-host' });

let composerKey = null;
let listKey = null;
let editingId = null;
let editInput = null; // kept across refreshes so an unsent edit survives
let plusMenu = null;

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
      peer: state.peer,
      title: name,
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
    titleBox.replaceChildren();
    tools.replaceChildren();
    list.replaceChildren(el('p', { class: 'empty', text: emptyText() }));
    composerHost.replaceChildren();
    composerKey = null;
    listKey = null;
    searchInput.value = '';
    state.ui.filter = '';
    return;
  }

  // A search applies to one conversation; switching clears it.
  const switched = view.key !== listKey;
  if (switched) state.ui.filter = '';
  if (searchInput.value !== state.ui.filter) searchInput.value = state.ui.filter;

  titleBox.replaceChildren(...titleNodes(view));
  tools.replaceChildren(...toolNodes(view));

  const shown = filtered(view.items);
  const wasAtBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 80;
  let rows;
  if (view.items.length === 0) rows = [el('p', { class: 'empty', text: 'No messages yet. Say hello.' })];
  else if (shown.length === 0) rows = [el('p', { class: 'empty', text: `No messages match "${state.ui.filter}".` })];
  else rows = rowsFor(shown, view);
  list.replaceChildren(...rows);
  if (switched || wasAtBottom) list.scrollTop = list.scrollHeight;
  listKey = view.key;

  // The composer is rebuilt only when the conversation changes, so a half-typed draft survives refreshes.
  if (view.key !== composerKey) {
    composerHost.replaceChildren(composer(view));
    composerKey = view.key;
  }
}

function titleNodes(view) {
  if (view.dm) {
    return [avatar(view.title, view.peer), el('h2', { text: view.title })];
  }
  return [icon('hash'), el('h2', { text: view.title })];
}

function iconButton(name, label, onclick, on = false) {
  return el('button', {
    class: `icon-btn${on ? ' on' : ''}`,
    'aria-label': label,
    title: label,
    'aria-pressed': on ? 'true' : 'false',
    icon: name,
    onclick,
  });
}

function toolNodes(view) {
  if (view.dm) {
    const open = state.ui.profileOpen;
    return [iconButton('panel-right', open ? 'Hide profile panel' : 'Show profile panel', () => actions.toggleProfile(), open)];
  }
  const open = state.ui.membersOpen;
  return [iconButton('users', open ? 'Hide member list' : 'Show member list', () => actions.toggleMembers(), open)];
}

function filtered(items) {
  const q = state.ui.filter.trim().toLowerCase();
  if (!q) return items;
  return items.filter((item) =>
    (item.text ?? '').toLowerCase().includes(q) || (item.authorName ?? '').toLowerCase().includes(q));
}

// Adds date dividers and groups consecutive messages from one author into a single header.
function rowsFor(items, view) {
  const out = [];
  let prev = null;
  for (const item of items) {
    const newDay = !prev || dayKey(prev.ms) !== dayKey(item.ms);
    if (newDay) out.push(el('div', { class: 'date-divider', role: 'separator' }, el('span', { text: dayLabel(item.ms) })));
    const start = newDay || prev.author !== item.author || Number(item.ms) - Number(prev.ms) > GROUP_MS;
    out.push(messageRow(item, view, start));
    prev = item;
  }
  return out;
}

function messageRow(item, view, start) {
  const editing = editingId === item.id;
  const classes = ['msg', start ? 'start' : 'cont', item.pending ? 'pending' : ''].filter(Boolean).join(' ');
  const body = editing ? editForm(item) : bodyNode(item);
  const pending = item.pending ? el('span', { class: 'pending-note', icon: 'clock', text: 'sending…' }) : null;
  const gutter = start
    ? el('div', { class: 'gutter' }, avatar(item.authorName, item.author, 'lg'))
    : el('div', { class: 'gutter' }, el('span', { class: 'gutter-time', text: clockLabel(item.ms) }));
  const content = start
    ? el('div', { class: 'msg-content' },
      el('div', { class: 'msg-head' },
        authorButton(item),
        el('time', { title: stampLabel(item.ms), text: clockLabel(item.ms) })),
      body, pending)
    : el('div', { class: 'msg-content' }, body, pending);
  return el('article', { class: classes, 'data-id': item.id }, gutter, content, view.dm || editing ? null : rowActions(item));
}

function authorButton(item) {
  const btn = el('button', {
    class: 'author',
    text: item.authorName || 'unknown',
    onclick: (e) => openProfile(item.author, e.clientX, e.clientY),
  });
  setHue(btn, item.author);
  return btn;
}

function bodyNode(item) {
  if (item.deleted) return el('p', { class: 'body deleted', text: 'message deleted' });
  return el('p', { class: 'body' },
    linkify(item.text),
    item.edited ? el('span', { class: 'edited', text: ' (edited)' }) : null);
}

function rowActions(item) {
  if (item.deleted) return null;
  const buttons = [];
  if (item.own) {
    buttons.push(el('button', {
      class: 'icon-btn',
      'aria-label': 'Edit message',
      title: 'Edit',
      icon: 'pencil',
      onclick: () => {
        editingId = item.id;
        editInput = null;
        render();
      },
    }));
  }
  if (item.own || canModerate()) {
    buttons.push(el('button', {
      class: 'icon-btn danger-text',
      'aria-label': 'Delete message',
      title: 'Delete',
      icon: 'trash-2',
      onclick: () => removeMessage(item.id),
    }));
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

// ---- composer ----

function composer(view) {
  const input = el('textarea', { rows: 1, placeholder: view.placeholder, 'aria-label': 'Message' });
  const wrap = el('div', { class: 'composer' });
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
  wrap.append(
    el('button', {
      class: 'icon-btn',
      'aria-label': 'More actions',
      'aria-haspopup': 'menu',
      title: 'More actions',
      icon: 'plus',
      onclick: (e) => togglePlusMenu(wrap, view, e.currentTarget),
    }),
    input,
    el('button', { class: 'icon-btn send', 'aria-label': 'Send message', title: 'Send', icon: 'send-horizontal', onclick: send }),
  );
  return wrap;
}

// Server: invite. DM: copy my contact link, so the other person can add me without a shared server.
function togglePlusMenu(wrap, view, anchor) {
  if (plusMenu) {
    closePlusMenu();
    return;
  }
  const item = (label, iconName, onPick) => el('button', {
    class: 'menu-item',
    role: 'menuitem',
    icon: iconName,
    text: label,
    onclick: () => {
      closePlusMenu();
      onPick();
    },
  });
  plusMenu = el('div', { class: 'menu plus-menu', role: 'menu' },
    view.dm
      ? item('Copy contact link', 'link', async () => {
        const link = await actions.contactLink();
        if (link && (await copyText(link))) toast('Contact link copied', 'info');
      })
      : item('Create invite', 'user-plus', () => openInviteDialog(state.community.id)));
  anchor.setAttribute('aria-expanded', 'true');
  wrap.append(plusMenu);
  document.addEventListener('pointerdown', onPlusOutside, true);
}

function onPlusOutside(e) {
  if (plusMenu && !plusMenu.contains(e.target) && !e.target.closest?.('[aria-haspopup="menu"]')) closePlusMenu();
}

function closePlusMenu() {
  plusMenu?.remove();
  plusMenu = null;
  document.removeEventListener('pointerdown', onPlusOutside, true);
}
