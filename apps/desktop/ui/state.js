// Single source of UI state. Actions mutate `state` and call notify() with the parts that changed.
export const state = {
  me: null,
  version: '',
  communities: [], // CommunitySummary[]
  conversations: [], // ConversationSummary[] (kind 'dm' or, with the groups feature, 'group')
  route: { kind: 'home' }, // { kind: 'home' } | { kind: 'server', communityId }
  community: null, // CommunityView of the open server
  channelId: null,
  messages: [], // MessageItem[] of the open channel
  peer: null, // root id of the open DM, or the group id of the open group
  peerName: '',
  peerProfile: null, // UserView of the open DM peer, for the profile panel
  group: null, // ConversationSummary of the open group, when `peer` is a group
  dmMessages: [], // DmItem[] of the open DM or group
  historyStart: false, // true when the loaded history reaches the first message
  loading: { messages: false }, // shows skeleton rows while a conversation loads
  unread: new Set(), // keys of channels and DMs with newer messages
  newFromId: null, // first message after the last-seen point, for the divider
  update: null, // UpdateInfo of a newer release, when one was found
  updateDismissed: false, // the user chose Later; the icon returns on the next check
  // Feature surfaces, filled only when the backend reports the feature.
  contacts: null, // { contacts, incoming, outgoing, blocked } from list_contacts
  ignored: new Set(), // root ids of ignored users, from ignored_users
  transfers: [], // TransferItem[] from list_transfers
  transferRates: null, // { down_bps, up_bps }
  call: null, // CallState while in a call, else null
  voiceChannelId: null, // the voice channel whose call board is open (server route)
  ui: {
    membersOpen: true, // server member list, right column
    profileOpen: true, // DM profile panel, right column
    filter: '', // client-side message search
    page: null, // 'contacts' | 'transfers' | null: pages that replace the chat pane
    transferId: null, // transfer whose details panel is open in the right column
    callChatOpen: true, // the chat strip under the call tiles
    callDetailsOpen: true, // the call details column
  },
};

const subscribers = new Map();

export function subscribe(part, fn) {
  if (!subscribers.has(part)) subscribers.set(part, []);
  subscribers.get(part).push(fn);
}

export function notify(...parts) {
  for (const part of parts) for (const fn of subscribers.get(part) ?? []) fn();
}

export const inServer = () => state.route.kind === 'server' && state.community !== null;
export const myRole = () => state.community?.my_role ?? null;
export const canModerate = () => ['owner', 'admin', 'moderator'].includes(myRole());
export const canManage = () => ['owner', 'admin'].includes(myRole());
export const inGroup = () => state.route.kind === 'home' && state.group !== null && state.peer === state.group.id;
