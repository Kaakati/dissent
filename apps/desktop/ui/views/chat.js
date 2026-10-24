// Main pane: header, message list and composer. The same markup serves channels and DMs.
import { state, subscribe, canModerate, inServer } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, icon, linkify, dayKey, dayLabel, clockLabel, stampLabel, setHue, toast, copyText, skeletonRows } from '../ui.js';
import { confirmDialog, openInviteDialog } from './dialogs.js';
import { openProfile } from './profile.js';
import { openEmojiPicker } from './emoji.js';
import { attachmentNode, stagedChip, dropOverlay, isOverLimit, baseName, guessMime, MAX_FILE_LABEL } from './attachments.js';
import { listenTo } from '../api.js';

const GROUP_MS = 7 * 60 * 1000; // consecutive messages by one author within this gap share a header
const FLASH_MS = 1200; // how long a jumped-to message stays highlighted

// The header and composer keep their DOM across renders, so the search box keeps focus while typing.
const titleBox = el('div', { class: 'chat-title' });
const tools = el('div', { class: 'header-tools' });
const searchInput = el('input', { type: 'search', placeholder: 'Search', 'aria-label': 'Search messages', autocomplete: 'off' });
searchInput.addEventListener('input', () => actions.setFilter(searchInput.value));
const header = el('header', { class: 'chat-header' }, titleBox, tools, el('div', { class: 'update-host', id: 'update-host' }), el('div', { class: 'search' }, searchInput, icon('search')));
const list = el('div', { class: 'messages', role: 'log', 'aria-label': 'Messages', 'aria-live': 'polite', tabindex: '0' });
const composerHost = el('div', { class: 'composer-host' });

let composerKey = null;
let listKey = null;
let editingId = null;
let editInput = null; // kept across refreshes so an unsent edit survives
let plusMenu = null;
let replyTo = null; // { id, authorName } while composing a reply
let composerInput = null;
let replyBar = null;
let staged = []; // files waiting in the composer: { path, name, size, mime }
let stagedRow = null;
let sendButton = null;
const overlay = dropOverlay();

export function initChat() {
  const main = document.getElementById('main');
  main.replaceChildren(header, list, composerHost, overlay);
  list.addEventListener('keydown', onListKey);
  initDrop(main);
  subscribe('main', render);
  render();
}

// ---- drag and drop ----

const hasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes('Files');
const canAttach = () => Boolean(current()?.files);

function showDrop(on) {
  const main = document.getElementById('main');
  if (on) overlay.setTitle(`Drop to send to ${current()?.dm ? current().title : `#${current()?.title ?? ''}`}`);
  main.classList.toggle('dropping', on);
}

// The backend owns drag and drop: it sends dissent://drag-over while files hover the window, dissent://drag-leave
// when they leave or after a drop, and dissent://files-dropped with { path, name, size, mime } per file. The mock
// translates the browser's drag events into the same three events, so this is the only code path.
function initDrop(main) {
  main.addEventListener('dragover', (e) => {
    if (hasFiles(e)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = canAttach() ? 'copy' : 'none';
    }
  });
  main.addEventListener('drop', (e) => {
    if (hasFiles(e)) e.preventDefault(); // never let the window navigate to a dropped file
  });
  listenTo('dissent://drag-over', () => { if (canAttach()) showDrop(true); }).catch(() => {});
  listenTo('dissent://drag-leave', () => showDrop(false)).catch(() => {});
  listenTo('dissent://files-dropped', (files) => {
    showDrop(false);
    if (!canAttach() || !Array.isArray(files)) return;
    stageFiles(files.filter((f) => f && typeof f.path === 'string').map((f) => ({
      path: f.path, name: f.name ?? baseName(f.path), size: f.size ?? null, mime: f.mime ?? guessMime(f.name ?? f.path),
    })));
  }).catch(() => {});
}

function stageFiles(files) {
  const known = new Set(staged.map((f) => f.path));
  for (const f of files) if (f && f.path && !known.has(f.path)) staged.push(f);
  renderStaged();
  composerInput?.focus();
}

function renderStaged() {
  if (!stagedRow) return;
  stagedRow.hidden = staged.length === 0;
  stagedRow.replaceChildren(...staged.map((f) => stagedChip(f, () => {
    staged = staged.filter((x) => x !== f);
    renderStaged();
    composerInput?.focus();
  })));
  if (sendButton) sendButton.disabled = staged.some(isOverLimit);
}

// Keyboard in the message list: Up and Down move focus between messages, R replies to the focused one.
// A focused message shows its action bar, so Tab then reaches those buttons.
function onListKey(e) {
  const focused = e.target.closest?.('.msg');
  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
    if (e.target !== list && !(focused && e.target === focused)) return;
    const rows = [...list.querySelectorAll('.msg')];
    if (!rows.length) return;
    const i = rows.indexOf(focused);
    const next = e.key === 'ArrowDown' ? (i < 0 ? 0 : Math.min(rows.length - 1, i + 1)) : (i < 0 ? rows.length - 1 : Math.max(0, i - 1));
    e.preventDefault();
    rows[next].focus();
    return;
  }
  if (e.key.toLowerCase() === 'r' && !e.ctrlKey && !e.metaKey && !e.altKey && focused && e.target === focused) {
    const view = current();
    const item = view?.items.find((m) => m.id === focused.dataset.id);
    if (view?.social && item && !item.deleted) {
      e.preventDefault();
      startReply(item);
    }
  }
}

// Normalises the open channel or DM into one shape the renderer understands.
function current() {
  const me = state.me;
  if (inServer()) {
    const channel = state.community.channels.find((c) => c.id === state.channelId);
    if (!channel) return null;
    const items = state.messages.map((m) => ({
      id: m.id,
      author: m.author,
      authorName: m.author_name,
      text: m.text,
      ms: m.ms,
      edited: m.edited,
      deleted: m.deleted,
      own: m.author === me?.root_id,
      pending: false,
      replyTo: m.reply_to ?? null,
      reactions: Array.isArray(m.reactions) ? m.reactions : null,
      attachments: Array.isArray(m.attachments) ? m.attachments : null,
    }));
    return {
      key: `ch:${state.community.id}:${channel.id}`,
      dm: false,
      title: channel.name,
      hash: true,
      placeholder: `Message #${channel.name}`,
      items,
      // Replies and reactions exist only when the backend sends a `reactions` array on messages; files, an `attachments` array.
      social: items.some((m) => m.reactions !== null),
      files: items.some((m) => m.attachments !== null),
    };
  }
  if (state.route.kind === 'home' && state.peer) {
    const name = peerName();
    const items = state.dmMessages.map((m) => {
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
        replyTo: null,
        reactions: null,
        attachments: Array.isArray(m.attachments) ? m.attachments : null,
      };
    });
    return {
      key: `dm:${state.peer}`,
      dm: true,
      peer: state.peer,
      title: name,
      placeholder: `Message ${name}`,
      items,
      social: false,
      files: items.some((m) => m.attachments !== null),
    };
  }
  return null;
}

function peerName() {
  const known = state.conversations.find((c) => c.peer === state.peer)?.display_name;
  return known || state.peerName || `anon·${state.peer.slice(0, 8)}`;
}

function emptyNode(iconName, title, text) {
  return el('div', { class: 'empty' }, icon(iconName), el('p', { class: 'empty-title', text: title }), text ? el('p', { class: 'muted', text }) : null);
}

function emptyPane() {
  if (state.route.kind === 'server') {
    return state.community
      ? emptyNode('hash', 'Pick a channel', 'Choose a channel on the left to start reading.')
      : emptyNode('hash', 'Loading server…', null);
  }
  return emptyNode('message-circle', 'Your direct messages', 'Pick a conversation, or add a contact to start one.');
}

function render() {
  const view = current();
  if (!view) {
    titleBox.replaceChildren();
    tools.replaceChildren();
    list.setAttribute('aria-busy', state.route.kind === 'server' && !state.community ? 'true' : 'false');
    list.replaceChildren(...(state.route.kind === 'server' && !state.community ? skeletonRows('message', 6) : [emptyPane()]));
    composerHost.replaceChildren();
    composerKey = null;
    listKey = null;
    searchInput.value = '';
    state.ui.filter = '';
    return;
  }

  // A search applies to one conversation; switching clears it, and drops a reply in progress.
  const switched = view.key !== listKey;
  if (switched) {
    state.ui.filter = '';
    replyTo = null;
    staged = [];
  }
  if (searchInput.value !== state.ui.filter) searchInput.value = state.ui.filter;

  titleBox.replaceChildren(...titleNodes(view));
  tools.replaceChildren(...toolNodes(view));

  const shown = filtered(view.items);
  const wasAtBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 80;
  let rows;
  list.setAttribute('aria-busy', state.loading.messages ? 'true' : 'false');
  if (state.loading.messages && view.items.length === 0) rows = skeletonRows('message', 6);
  else if (view.items.length === 0) rows = [emptyNode(view.dm ? 'message-circle' : 'hash', 'No messages yet', 'Say hello.')];
  else if (shown.length === 0) rows = [emptyNode('search', 'No matches', `No messages match "${state.ui.filter}".`)];
  else rows = rowsFor(shown, view);
  const head = state.historyStart && !state.ui.filter ? [welcomeNode(view)] : [];
  list.replaceChildren(...head, ...rows);
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
    return [avatar(view.title, view.peer, 'sm'), el('h2', { text: view.title, title: view.title })];
  }
  return [icon('hash'), el('h2', { text: view.title, title: `#${view.title}` })];
}

function iconButton(name, label, onclick, on = false) {
  return el('button', {
    class: `icon-btn lg${on ? ' on' : ''}`,
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
    const start = newDay || prev.author !== item.author || Number(item.ms) - Number(prev.ms) > GROUP_MS || Boolean(item.replyTo);
    if (item.id === state.newFromId) {
      out.push(el('div', { class: 'new-divider', role: 'separator' }, el('span', { text: 'New messages' })));
    }
    out.push(messageRow(item, view, start || item.id === state.newFromId));
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
  const reactions = view.social && item.reactions?.length && !item.deleted ? reactionsNode(item) : null;
  const files = view.files && item.attachments?.length && !item.deleted
    ? el('div', { class: 'attachments' }, item.attachments.map((a) => attachmentNode(a, { messageId: item.id })))
    : null;
  const bodyOrNull = item.deleted || item.text || !files ? body : null; // a file-only message has no empty text line
  const content = start
    ? el('div', { class: 'msg-content' },
      el('div', { class: 'msg-head' },
        authorButton(item),
        el('time', { title: stampLabel(item.ms), text: clockLabel(item.ms) })),
      bodyOrNull, files, pending, reactions)
    : el('div', { class: 'msg-content' }, bodyOrNull, files, pending, reactions);
  return el('article', { class: classes, 'data-id': item.id, tabindex: '-1' },
    item.replyTo ? replyRef(item.replyTo) : null,
    gutter, content, view.dm || editing ? null : rowActions(item, view));
}

// ---- replies ----

// The line above a reply: a connector from the avatar column, the original's author and a one-line snippet.
function replyRef(ref) {
  const missing = ref.text === null || ref.text === undefined;
  const label = missing
    ? (ref.locked ? 'Original message is encrypted' : 'Original message was deleted')
    : `Replying to ${ref.author_name}: ${ref.text}`;
  return el('button', {
    class: 'reply-ref',
    title: label,
    'aria-label': label,
    onclick: () => jumpTo(ref.id),
  },
  missing
    ? el('span', { class: 'reply-missing', text: ref.locked ? 'Original message is encrypted' : 'Original message was deleted' })
    : [
      avatar(ref.author_name, ref.author ?? ref.author_name, 'xs'),
      el('span', { class: 'reply-author', text: ref.author_name || 'unknown' }),
      el('span', { class: 'reply-snippet', text: ref.text }),
    ]);
}

// Scrolls to a message and flashes it. The flash is an opacity animation on the row's overlay.
function jumpTo(messageId) {
  const node = [...list.querySelectorAll('.msg')].find((n) => n.dataset.id === messageId);
  if (!node) {
    toast('The original message is not in the loaded history.', 'info');
    return;
  }
  node.scrollIntoView({ block: 'center' });
  node.focus();
  node.classList.remove('flash');
  void node.offsetWidth; // restart the animation when flashing the same row twice
  node.classList.add('flash');
  setTimeout(() => node.classList.remove('flash'), FLASH_MS);
}

function startReply(item) {
  replyTo = { id: item.id, authorName: item.authorName || 'unknown' };
  renderReplyBar();
  composerInput?.focus();
}

function cancelReply() {
  if (!replyTo) return;
  replyTo = null;
  renderReplyBar();
}

function renderReplyBar() {
  if (!replyBar) return;
  replyBar.hidden = !replyTo;
  if (!replyTo) return;
  replyBar.replaceChildren(
    el('span', { class: 'reply-bar-text' }, 'Replying to ', el('strong', { text: replyTo.authorName })),
    el('button', { class: 'icon-btn', 'aria-label': 'Cancel reply', title: 'Cancel reply (Esc)', icon: 'x', onclick: () => { cancelReply(); composerInput?.focus(); } }),
  );
}

// ---- reactions ----

function reactionsNode(item) {
  const pills = item.reactions.map((r) => {
    const who = whoReacted(r);
    return el('button', {
      class: `reaction${r.me ? ' me' : ''}`,
      'aria-pressed': r.me ? 'true' : 'false',
      'aria-label': `${r.emoji} ${r.count}, ${who}`,
      title: who,
      onclick: () => actions.toggleReaction(item.id, r.emoji, Boolean(r.me)),
    },
    el('span', { class: 'reaction-emoji', text: r.emoji }),
    el('span', { class: 'reaction-count', text: String(r.count) }));
  });
  pills.push(el('button', {
    class: 'reaction add',
    'aria-label': 'Add reaction',
    title: 'Add reaction',
    icon: 'smile-plus',
    onclick: (e) => pickReaction(item, e.currentTarget),
  }));
  return el('div', { class: 'reactions', role: 'group', 'aria-label': 'Reactions' }, pills);
}

// "Grace, Ken and 2 others". `names` is optional in the contract; without it the count and `me` are used.
function whoReacted(r) {
  const count = Number(r.count) || 0;
  const names = Array.isArray(r.names) ? r.names.filter(Boolean).slice(0, 3) : [];
  if (names.length === 0) {
    if (r.me) return count > 1 ? `You and ${count - 1} other${count === 2 ? '' : 's'}` : 'You';
    return `${count} reaction${count === 1 ? '' : 's'}`;
  }
  const rest = count - names.length;
  if (rest > 0) {
    const shown = names.length > 2 ? names.slice(0, 2) : names;
    const others = count - shown.length;
    return `${shown.join(', ')} and ${others} other${others === 1 ? '' : 's'}`;
  }
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

function pickReaction(item, anchor) {
  openEmojiPicker(anchor, (emoji) => {
    const mine = item.reactions?.some((r) => r.emoji === emoji && r.me) ?? false;
    actions.toggleReaction(item.id, emoji, mine);
  });
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

function rowActions(item, view) {
  if (item.deleted) return null;
  const buttons = [];
  if (view.social) {
    buttons.push(el('button', {
      class: 'icon-btn',
      'aria-label': 'Reply',
      title: 'Reply (R)',
      icon: 'reply',
      onclick: () => startReply(item),
    }));
    buttons.push(el('button', {
      class: 'icon-btn',
      'aria-label': 'Add reaction',
      title: 'Add reaction',
      'aria-haspopup': 'dialog',
      icon: 'smile-plus',
      onclick: (e) => pickReaction(item, e.currentTarget),
    }));
  }
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
  composerInput = input;
  replyBar = el('div', { class: 'reply-bar', role: 'status', hidden: true });
  renderReplyBar();
  stagedRow = el('div', { class: 'staged', role: 'list', 'aria-label': 'Files to send', hidden: true });
  const send = async () => {
    const text = input.value.trim();
    const files = staged;
    if (!text && files.length === 0) return;
    if (files.some(isOverLimit)) {
      toast(`Remove files over ${MAX_FILE_LABEL} before sending.`);
      return;
    }
    input.disabled = true;
    const reply = replyTo;
    let ok;
    if (files.length) ok = await actions.sendFiles(files.map((f) => f.path), text || null);
    else if (view.dm) ok = await actions.sendDm(text);
    else if (reply) ok = await actions.sendReply(reply.id, text);
    else ok = await actions.sendMessage(text);
    input.disabled = false;
    if (ok) {
      input.value = '';
      if (staged === files) {
        staged = [];
        renderStaged();
      }
      if (replyTo === reply) cancelReply();
    }
    input.focus();
  };
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    } else if (e.key === 'Escape' && replyTo) {
      e.preventDefault();
      cancelReply();
    }
  });
  wrap.append(
    el('button', {
      class: 'icon-btn lg',
      'aria-label': 'More actions',
      'aria-haspopup': 'menu',
      title: 'More actions',
      icon: 'plus',
      onclick: (e) => togglePlusMenu(wrap, view, e.currentTarget),
    }),
    input,
    sendButton = el('button', { class: 'icon-btn lg send', 'aria-label': 'Send message', title: 'Send', icon: 'send-horizontal', onclick: send }),
  );
  renderStaged();
  return el('div', { class: 'composer-stack' }, replyBar, stagedRow, wrap);
}

// The native file dialog, through the backend. Picked files are staged, not sent.
async function uploadFile() {
  const picked = await actions.pickFiles();
  if (!Array.isArray(picked) || picked.length === 0) return;
  stageFiles(picked.map((f) => ({ path: f.path, name: f.name ?? baseName(f.path), size: f.size ?? null, mime: f.mime ?? guessMime(f.name ?? f.path) })));
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
  // The composer outlives message loads, so feature flags are read from the live view, not the one it was built with.
  const live = current() ?? view;
  plusMenu = el('div', { class: 'menu plus-menu', role: 'menu' },
    live.files ? item('Upload a file', 'paperclip', uploadFile) : null,
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

// The top of a history that has no older messages: channel welcome, or DM history start.
function welcomeNode(view) {
  if (view.dm) {
    const mutual = state.peerProfile?.mutual_communities.length ?? 0;
    const suffix = state.peerProfile?.suffix ?? view.peer.slice(0, 8);
    return el('div', { class: 'welcome' },
      avatar(view.title, view.peer, 'xl'),
      el('h2', { text: view.title }),
      el('div', { class: 'suffix mono', text: `${view.title}·${suffix}` }),
      el('p', { class: 'muted', text: `This is the beginning of your direct message history with ${view.title}.` }),
      el('p', { class: 'muted', text: mutual ? `${mutual} mutual server${mutual === 1 ? '' : 's'}` : 'No mutual servers' }));
  }
  return el('div', { class: 'welcome' },
    el('div', { class: 'welcome-mark' }, icon('hash')),
    el('h2', { text: `Welcome to #${view.title}!` }),
    el('p', { class: 'muted', text: `This is the start of the #${view.title} channel.` }));
}
