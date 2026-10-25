// The Friends page: the default view of Home. Header tabs (Online, All, Pending, and Blocked when there are blocked
// people) with Add Friend; the list of friends with presence, status and actions; and the Active Now aside.
// The data is list_contacts(). "Friends" is display wording only.
import { state, notify, voiceChannelsOf } from '../state.js';
import * as actions from '../actions.js';
import { el, icon, avatar, copyText, toast, presenceOf, PRESENCE_LABEL } from '../ui.js';
import { addressOf, isAddressToAdd, matchesSearch, shortOf } from '../address.js';
import { openConversationMenu } from './sidebar.js';

let tab = 'online';
let query = '';
let addOpen = false;

const EMPTY = { incoming: [], outgoing: [], contacts: [], blocked: [] };

// Presence of a person: the status when the backend gives one, else online or offline. Unknown counts as offline.
const presenceOfPerson = (p) => presenceOf(p) ?? 'offline';
const isUp = (p) => presenceOfPerson(p) !== 'offline';

// Where a friend is in a call right now: a group or direct call on a conversation, or a voice channel of the open server.
function callOf(rootId) {
  for (const conv of state.conversations) {
    const live = conv.live_call;
    if (!live) continue;
    const inCall = live.participants?.some((m) => m.root_id === rootId) || (live.scope === 'dm' && conv.peer === rootId);
    if (inCall) return conv.kind === 'group' ? conv.name : 'a direct call';
  }
  if (state.community) {
    for (const ch of voiceChannelsOf(state.community)) {
      if (ch.participants?.some((m) => m.root_id === rootId)) return `#${ch.name} · ${state.community.name}`;
    }
  }
  return null;
}

// A stopper for buttons inside a row: their click must not also open the DM the row opens.
const stop = (fn) => (e) => { e.stopPropagation(); fn(e); };

// The conversation-shaped object the DM menu expects, for someone who has no conversation row yet.
const dmOf = (p) => ({ kind: 'dm', peer: p.root_id, display_name: p.display_name, pinned: false });

function friendRow(p) {
  const open = () => actions.openDm(p.root_id, p.display_name);
  const call = callOf(p.root_id);
  const sub = call ? `In a call · ${call}` : PRESENCE_LABEL[presenceOfPerson(p)];
  return el('div', { class: 'friend-row', onclick: open },
    avatar(p.display_name, p.root_id, '', presenceOfPerson(p)),
    personText(p, sub),
    el('div', { class: 'actions' },
      el('button', { class: 'icon-btn round', 'aria-label': `Message ${p.display_name}`, title: 'Message', icon: 'message-circle', onclick: stop(open) }),
      el('button', {
        class: 'icon-btn round',
        'aria-label': `More actions for ${p.display_name}`,
        title: 'More',
        'aria-haspopup': 'menu',
        icon: 'ellipsis-vertical',
        onclick: stop((e) => {
          const r = e.currentTarget.getBoundingClientRect();
          openConversationMenu(r.left, r.bottom, dmOf(p));
        }),
      })));
}

function incomingRow(p) {
  return el('div', { class: 'friend-row pending' },
    avatar(p.display_name, p.root_id, '', presenceOfPerson(p)),
    personText(p, p.online ? 'Incoming request · Online' : 'Incoming request'),
    el('div', { class: 'actions' },
      el('button', { class: 'btn-sm ok', 'aria-label': `Accept ${p.display_name}`, text: 'Accept', onclick: () => actions.acceptContact(p.root_id) }),
      el('button', { class: 'btn-sm', 'aria-label': `Ignore ${p.display_name}`, text: 'Ignore', onclick: () => actions.declineContact(p.root_id) })));
}

function outgoingRow(p) {
  return el('div', { class: 'friend-row pending' },
    avatar(p.display_name, p.root_id, '', presenceOfPerson(p)),
    personText(p, 'Outgoing request · Waiting for them'),
    el('div', { class: 'actions' },
      el('button', { class: 'btn-sm', 'aria-label': `Cancel request to ${p.display_name}`, text: 'Cancel', onclick: () => actions.cancelContactRequest(p.root_id) })));
}

function blockedRow(p) {
  return el('div', { class: 'friend-row pending' },
    avatar(p.display_name, p.root_id, '', 'offline'),
    personText(p, 'Blocked'),
    el('div', { class: 'actions' },
      el('button', { class: 'btn-sm', 'aria-label': `Unblock ${p.display_name}`, text: 'Unblock', onclick: () => actions.unblockUser(p.root_id) })));
}

// The name is bold and the tag shows on hover (CSS); the second line is the status.
function personText(p, sub) {
  return el('div', { class: 'text' },
    el('div', { class: 'line' },
      el('span', { class: 'name', text: p.display_name }),
      el('span', { class: 'tag', text: `·${shortOf('user', p.root_id)}`, title: addressOf('user', p.root_id) })),
    el('div', { class: 'sub', text: sub }));
}

function tabsFor(data) {
  const tabs = [['online', 'Online'], ['all', 'All'], ['pending', 'Pending']];
  if ((data.blocked ?? []).length) tabs.push(['blocked', 'Blocked']);
  return tabs;
}

// The rows of the active tab, under one section title, filtered by the search box.
function sectionsFor(data) {
  const q = query.trim().toLowerCase();
  const match = (p) => matchesSearch(p, q);
  const list = (arr) => (arr ?? []).filter(match);
  if (tab === 'pending') {
    const rows = [...list(data.incoming).map(incomingRow), ...list(data.outgoing).map(outgoingRow)];
    return rows.length ? [{ title: `Pending — ${rows.length}`, rows }] : [];
  }
  if (tab === 'blocked') {
    const rows = list(data.blocked).map(blockedRow);
    return rows.length ? [{ title: `Blocked — ${rows.length}`, rows }] : [];
  }
  const people = list(data.contacts);
  const picked = tab === 'online' ? people.filter(isUp) : people;
  const title = tab === 'online' ? 'Online' : 'All friends';
  return picked.length ? [{ title: `${title} — ${picked.length}`, rows: picked.map(friendRow) }] : [];
}

const EMPTY_COPY = {
  online: ['Nobody’s online', 'Friends appear here when their app is running.'],
  all: ['No friends yet', 'Add someone with their contact link.'],
  pending: ['No pending requests', 'Requests you send or receive show up here.'],
  blocked: ['Nobody is blocked', 'People you block can’t message you or send requests.'],
};

function emptyFor(key) {
  const [title, text] = EMPTY_COPY[key];
  return el('div', { class: 'page-empty' }, icon('users'), el('span', { class: 't', text: title }), el('span', { text }));
}

// Repaints only the list, so the search box keeps its focus while typing.
function paintList(host, data) {
  const sections = sectionsFor(data);
  const incoming = data.incoming?.length ?? 0;
  const tip = tab === 'pending' && incoming > 0
    ? el('div', { class: 'page-tip' }, icon('info'), el('span', { text: 'Names can repeat. Check the tag after the dot matches the person you know before you accept.' }))
    : null;
  host.replaceChildren(tip, ...(sections.length
    ? sections.map((s) => el('section', { class: 'list-section' }, el('h3', { class: 'list-head', text: s.title }), el('div', { class: 'friend-list' }, s.rows)))
    : [emptyFor(tab)]));
}

function addBox() {
  const input = el('input', { placeholder: 'dissent://contact/…', 'aria-label': 'Contact link', spellcheck: false });
  const status = el('p', { class: 'muted', role: 'status' });
  const sendBtn = el('button', { class: 'primary', text: 'Send Friend Request', onclick: () => send() });
  const send = async () => {
    const link = input.value.trim();
    if (!link) return;
    let byAddress = false;
    try {
      byAddress = isAddressToAdd(link);
    } catch (e) {
      toast(e.message);
      return;
    }
    sendBtn.disabled = true;
    status.textContent = byAddress ? 'Looking them up…' : '';
    const sent = await actions.sendContactRequest(link);
    sendBtn.disabled = false;
    status.textContent = '';
    if (sent) {
      input.value = '';
      addOpen = false;
      tab = 'pending';
      notify('main');
    }
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') send(); });
  const linkBox = el('div', { class: 'link-box', text: '…' });
  actions.contactLink().then((link) => { if (link) linkBox.textContent = link; }).catch(() => {});
  return el('div', { class: 'add-friend-form' },
    el('div', { class: 'add-head' },
      el('h3', { text: 'Add Friend' }),
      el('p', { text: 'Paste someone’s address or contact link. They’ll see your request and can accept or decline it. Nothing is sent through a company server.' }),
      el('div', { class: 'add-contact-box' }, input, sendBtn),
      status),
    el('div', { class: 'add-share' },
      el('h3', { text: 'Share yours' }),
      el('p', { text: 'Send it any way you like. People who open it can request to add you, and you decide who gets in.' }),
      linkBox,
      el('button', { class: 'primary', icon: 'copy', text: 'Copy contact link', onclick: async () => {
        const link = await actions.contactLink();
        if (link && (await copyText(link))) toast('Contact link copied', 'info');
      } })));
}

// Active Now: friends in a call first, then the rest who are online. Quiet when nobody is.
function activeAside(data) {
  const friends = data.contacts ?? [];
  const inCall = friends.filter((p) => callOf(p.root_id));
  const online = friends.filter((p) => isUp(p) && !callOf(p.root_id));
  const card = (p, sub) => el('button', { class: 'active-card', onclick: () => actions.openDm(p.root_id, p.display_name) },
    avatar(p.display_name, p.root_id, '', presenceOfPerson(p)),
    el('span', { class: 'text' },
      el('span', { class: 'name', text: p.display_name }),
      el('span', { class: 'sub', text: sub })));
  const cards = [
    ...inCall.map((p) => card(p, `In a call · ${callOf(p.root_id)}`)),
    ...online.map((p) => card(p, PRESENCE_LABEL[presenceOfPerson(p)])),
  ];
  return el('aside', { class: 'page-aside', 'aria-label': 'Active now' },
    el('h3', { text: 'Active Now' }),
    cards.length
      ? el('div', { class: 'active-list' }, cards)
      : el('div', { class: 'quiet-box' },
        el('div', { class: 't', text: 'It’s quiet for now…' }),
        el('div', { class: 's', text: 'When a friend joins a call or comes online, they’ll show up here.' })));
}

export function contactsPage() {
  const data = state.contacts ?? EMPTY;
  if (!tabsFor(data).some(([key]) => key === tab)) tab = 'online';
  const incoming = data.incoming?.length ?? 0;
  const tablist = el('div', { class: 'page-tabs', role: 'tablist', 'aria-label': 'Friends' },
    tabsFor(data).map(([key, label]) => {
      const on = tab === key && !addOpen;
      return el('button', {
        class: `page-tab${on ? ' active' : ''}`,
        role: 'tab',
        'aria-selected': String(on),
        onclick: () => { tab = key; addOpen = false; notify('main'); },
      }, label, key === 'pending' && incoming ? el('span', { class: 'count', text: String(incoming), 'aria-label': `${incoming} incoming requests` }) : null);
    }));
  const addButton = el('button', {
    class: `add-friend${addOpen ? ' active' : ''}`,
    'aria-pressed': String(addOpen),
    onclick: () => { addOpen = true; notify('main'); },
    text: 'Add Friend',
  });
  const list = el('div', { class: 'friend-lists' });
  const search = el('input', { class: 'page-search', placeholder: 'Search', 'aria-label': 'Search friends', value: query });
  search.addEventListener('input', () => { query = search.value; paintList(list, state.contacts ?? EMPTY); });
  paintList(list, data);
  const body = addOpen
    ? el('div', { class: 'page-scroll' }, addBox())
    : el('div', { class: 'page-scroll friends-body' }, search, list);
  return {
    body,
    aside: activeAside(data),
    title: 'Friends',
    titleIcon: 'users',
    headerExtra: [el('span', { class: 'friends-sep', 'aria-hidden': 'true' }), tablist, addButton],
  };
}
