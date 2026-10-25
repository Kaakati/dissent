// Right column. Shows the server member list, or the DM profile panel, or nothing.
import { state, subscribe } from '../state.js';
import { membersNodes } from './members.js';
import { profilePanelNodes } from './dmpanel.js';
import * as actions from '../actions.js';
import { skeletonRows, avatar, el, copyText, toast, presenceOf } from '../ui.js';
import { addressOf, shortOf } from '../address.js';
import { confirmDialog } from './dialogs.js';
import { has } from '../features.js';
import { pageBuilders } from './pages.js';
import { fileDetailNodes } from './filedetail.js';
import { callDetailsNodes } from './call.js';
import { lobbyRosterNodes } from './lobby.js';

export function initRight() {
  subscribe('right', render);
  render();
}

function render() {
  const host = document.getElementById('right');
  const app = document.getElementById('app');
  let mode = 'none';
  let nodes = [];
  if (state.call?.scope === 'channel' && state.voiceChannelId) {
    mode = 'members';
    nodes = [el('div', { class: 'right-card' }, ...callDetailsNodes(state.call))];
  } else if (state.ui.lobby) {
    mode = 'members';
    nodes = [el('div', { class: 'right-card lobby-card' }, ...lobbyRosterNodes(state.ui.lobby.live))];
  } else if (state.ui.fileDetail && fileDetailNodes().length) {
    mode = 'members';
    nodes = fileDetailNodes();
  } else if (state.ui.page && pageBuilders[state.ui.page]) {
    // Friends has its own 340 px column for Active Now; the other pages keep the standard width.
    mode = state.ui.page === 'contacts' ? 'wide' : 'members';
    nodes = [pageBuilders[state.ui.page]().aside];
  } else if (state.route.kind === 'server' && state.community && state.ui.membersOpen) {
    mode = 'members';
    const count = state.community.members.filter((m) => !m.banned).length;
    nodes = [card(`Members — ${count}`, membersNodes(state.community))];
  } else if (state.route.kind === 'server' && state.ui.membersOpen) {
    mode = 'members';
    nodes = skeletonRows('member', 5);
  } else if (state.route.kind === 'home' && state.group && state.ui.membersOpen) {
    mode = 'members';
    nodes = [groupMembersCard(state.group)];
  } else if (state.route.kind === 'home' && state.peer && state.ui.profileOpen) {
    mode = 'profile';
    nodes = profilePanelNodes();
  }
  app.dataset.right = mode;
  host.hidden = mode === 'none';
  host.replaceChildren(...nodes.filter(Boolean));
}

// A rounded card with a header row and a scrolling body, as on the member-list boards.
function card(title, body) {
  return el('div', { class: 'right-card' },
    el('div', { class: 'right-head' }, el('h3', { text: title })),
    el('div', { class: 'right-scroll' }, body));
}


// Members of an open group (GroupDM board): a creator badge, a name·suffix tag under each name, and the invite and leave actions.
function groupMembersCard(group) {
  const members = group.members ?? [];
  const rows = members.map((m) => el('div', { class: 'member two-line', title: `${m.display_name}·${addressOf('user', m.root_id)}` },
    avatar(m.display_name, m.root_id, '', presenceOf(m)),
    el('span', { class: 'member-text' },
      el('span', { class: 'member-name' },
        el('span', { text: m.display_name }),
        m.root_id === group.creator ? el('span', { class: 'badge-soft', text: 'Creator' }) : null),
      el('span', { class: 'tag', text: `${m.display_name.toLowerCase()}·${shortOf('user', m.root_id)}` }))));
  const copyInvite = async () => {
    const link = await actions.groupInviteLink(group.id);
    if (link && (await copyText(link))) toast('Group invite link copied', 'info');
  };
  const leave = async () => {
    if (await confirmDialog(`Leave ${group.name}? You'll stop receiving its messages.`, 'Leave')) actions.leaveGroup(group.id);
  };
  return el('div', { class: 'right-card' },
    el('div', { class: 'right-head' }, el('h3', { text: `Members — ${members.length}` })),
    el('div', { class: 'right-scroll' }, rows),
    has('groups') ? el('div', { class: 'right-foot' },
      el('button', { text: 'Copy group invite link', onclick: copyInvite }),
      el('button', { class: 'danger-outline', text: 'Leave group', onclick: leave })) : null);
}
