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
  dmMessages: [], // DmItem[] of the open DM
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
export const handleOf = (rootId) => state.community?.members.find((m) => m.root_id === rootId)?.handle ?? null;
export const canModerate = () => ['owner', 'admin', 'moderator'].includes(myRole());
