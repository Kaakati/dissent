// Single source of UI state. Actions mutate `state` and call notify() with the parts that changed.
export const state = {
  me: null,
  version: '',
  communities: [], // CommunitySummary[]
  conversations: [], // ConversationSummary[]
  route: { kind: 'home' }, // { kind: 'home' } | { kind: 'server', communityId }
  community: null, // CommunityView of the open server
  channelId: null,
  messages: [], // MessageItem[] of the open channel
  peer: null, // root id of the open DM
  peerName: '',
  peerProfile: null, // UserView of the open DM peer, for the profile panel
  dmMessages: [], // DmItem[] of the open DM
  ui: {
    membersOpen: true, // server member list, right column
    profileOpen: true, // DM profile panel, right column
    filter: '', // client-side message search
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
