// Single source of shell state. Ported from the old UI's state.js and the navigation, status, call, transfer and engine-event
// actions in the old UI's actions.js. Views read the store; actions below call the backend and then update it. Message lists,
// composers and dialogs are not here yet: the batch that ports each view adds its slice.
import { create } from 'zustand';
import { api } from './api';
import { has } from './features';
import { markSeen, seenMs } from './unread';
import { toast } from './toasts';
import type {
  CallState,
  CommunitySummary,
  CommunityView,
  ContactsView,
  ConversationSummary,
  EngineEvent,
  GroupSummary,
  Id,
  Me,
  NetLatency,
  NetworkSettings,
  Page,
  Route,
  StatusName,
  TransferItem,
  TypingTarget,
  UpdateInfo,
  UserStatus,
  VoiceConnection,
  ShareSource,
} from './types';

// The key a conversation's unread marker and typing state use. Direct and group conversations differ in their id.
export const convKey = (conv: ConversationSummary): string => (conv.kind === 'group' ? `group:${conv.id}` : `dm:${conv.peer}`);
export const channelKey = (communityId: Id, channelId: Id): string => `ch:${communityId}:${channelId}`;

export function typingKey(target: TypingTarget): string {
  switch (target.kind) {
    case 'channel': return `channel|${target.communityId}|${target.channelId}`;
    case 'dm': return `dm|${target.peer}`;
    case 'group': return `group|${target.groupId}`;
  }
}

// A place the user can go back to: the view and the conversation or channel in it.
export type Place =
  | { kind: 'home' }
  | { kind: 'page'; page: Page }
  | { kind: 'server'; communityId: Id; channelId: Id | null }
  | { kind: 'dm'; peer: Id; name: string }
  | { kind: 'group'; id: Id };

const MAX_HISTORY = 50;

function placeOf(s: Pick<ShellState, 'page' | 'route' | 'channelId' | 'group' | 'peer' | 'peerName'>): Place {
  if (s.page) return { kind: 'page', page: s.page };
  if (s.route.kind === 'server') return { kind: 'server', communityId: s.route.communityId, channelId: s.channelId };
  if (s.group) return { kind: 'group', id: s.group.id };
  if (s.peer) return { kind: 'dm', peer: s.peer, name: s.peerName };
  return { kind: 'home' };
}

const placeKey = (p: Place): string => JSON.stringify(p);

// Set while Back or Forward replays a place, so the replay is not recorded as a new visit.
let replaying = false;

// Records a visit: the place before it goes on Back, and Forward is cleared. Nothing is recorded for a replay, or
// when the place did not change. Only completed navigations count, so the steps inside one are not entries.
async function navigate(fn: () => Promise<void>): Promise<void> {
  const from = placeOf(useShell.getState());
  await fn();
  const to = placeOf(useShell.getState());
  if (replaying || placeKey(from) === placeKey(to)) return;
  useShell.setState((s) => ({ back: [...s.back, from].slice(-MAX_HISTORY), forward: [] }));
}

// Opens a place without recording it.
function goTo(p: Place): Promise<void> {
  const shell = useShell.getState();
  switch (p.kind) {
    case 'home': return shell.openHome();
    case 'page': return shell.openPage(p.page);
    case 'server': return shell.openServer(p.communityId, p.channelId);
    case 'dm': return shell.openDm(p.peer, p.name);
    case 'group': {
      const conv = shell.conversations.find((c) => c.kind === 'group' && c.id === p.id);
      return conv?.kind === 'group' ? shell.openGroup(conv) : shell.openHome();
    }
  }
}

// Back and Forward: one entry moves from one stack to the other, and its place opens.
async function travel(direction: 'back' | 'forward'): Promise<void> {
  const s = useShell.getState();
  const source = direction === 'back' ? s.back : s.forward;
  const entry = source[source.length - 1];
  if (!entry) return;
  const here = placeOf(s);
  if (direction === 'back') useShell.setState({ back: source.slice(0, -1), forward: [...s.forward, here] });
  else useShell.setState({ forward: source.slice(0, -1), back: [...s.back, here] });
  replaying = true;
  try {
    await goTo(entry);
  } finally {
    replaying = false;
  }
}

export type SettingsSection = 'account' | 'recovery' | 'devices' | 'profile' | 'appearance' | 'sounds' | 'voice' | 'network' | 'updates';

export interface ShellState {
  me: Me | null;
  status: UserStatus; // your own status; until_ms is set when timed
  version: string;
  communities: CommunitySummary[];
  conversations: ConversationSummary[];
  route: Route;
  community: CommunityView | null; // the open server
  channelId: Id | null;
  voiceChannelId: Id | null; // the voice channel whose call board is open (server route)
  peer: Id | null; // root id of the open DM, or the group id of the open group
  peerName: string;
  group: GroupSummary | null; // the open group, when `peer` is a group
  page: Page | null; // pages that replace the chat pane
  unread: ReadonlySet<string>; // keys of channels and conversations with newer messages
  contacts: ContactsView | null;
  ignored: Id[]; // root ids of ignored users
  transfers: TransferItem[];
  transferRates: { down_bps: number; up_bps: number } | null;
  call: CallState | null; // set while in a call
  voiceConnection: VoiceConnection | null; // the voice channel this device is connected to
  back: Place[]; // places to return to, newest last
  forward: Place[]; // places to go forward to after Back, newest last
  update: UpdateInfo | null; // a newer release, when one was found
  updateDismissed: boolean; // "Later" hides the title bar's update button until the next release is found
  settings: SettingsSection | null; // the full-page Settings, open at this section
  onboarding: boolean; // the first-run identity screen covers the shell
  titleOverride: string | null; // the title bar reads this instead of the place (first run)
  typing: Record<string, string[]>; // names typing, keyed by typingKey()
  ready: boolean; // true once boot() has loaded the shell

  boot: () => Promise<void>;
  loadStatus: () => Promise<void>;
  loadCommunities: () => Promise<void>;
  moveServer: (communityId: Id, index: number) => Promise<void>;
  loadConversations: () => Promise<void>;
  loadCommunity: (communityId: Id) => Promise<void>;
  loadContacts: () => Promise<void>;
  loadIgnored: () => Promise<void>;
  loadTransfers: () => Promise<void>;
  loadCallState: () => Promise<void>;
  refreshUnread: () => Promise<void>;

  openHome: () => Promise<void>;
  openServer: (communityId: Id, channelId?: Id | null) => Promise<void>;
  openChannel: (channelId: Id) => Promise<void>;
  openPage: (page: Page) => Promise<void>;
  openDm: (peer: Id, name: string) => Promise<void>;
  // Drops what the UI keeps for a direct conversation that has left the list: its unread marker and typing names.
  clearDmState: (peer: Id) => void;
  openGroup: (conv: GroupSummary) => Promise<void>;
  openConversation: (conv: ConversationSummary) => Promise<void>;
  goBack: () => Promise<void>;
  goForward: () => Promise<void>;

  setStatus: (status: StatusName, untilMs: number | null) => Promise<void>;
  toggleMuted: () => Promise<void>;
  toggleDeafened: () => Promise<void>;
  toggleCamera: () => Promise<void>;
  /** Shares a display or window, or stops the share when one is running. */
  toggleScreenShare: (target?: ShareSource['target']) => Promise<void>;
  /** The latest preview of each person's camera (`root_id`) and shared screen (`root_id:screen`). JPEG, base64. */
  videoFrames: Record<string, { width: number; height: number; jpeg: string }>;
  leaveCall: () => Promise<void>;

  handleEngineEvent: (ev: EngineEvent) => Promise<void>;
  handleCallEvent: (call: CallState | null) => void;
  handleTransfersEvent: (payload: TransferItem[] | { transfers?: TransferItem[]; rates?: ShellState['transferRates'] }) => void;
  applyStatus: (payload: UserStatus) => void;
  applyTyping: (payload: { target: TypingTarget; names: string[] }) => void;
  setUpdate: (info: UpdateInfo) => void;
  avatarChanged: (payload: { community_id?: Id } | null) => Promise<void>;

  openSettings: (section?: SettingsSection) => void;
  closeSettings: () => void;
  // A device-link code from a dissent://link/ link, waiting for Settings › Devices to show it. Never approved by itself.
  pendingDeviceLink: string | null;
  // The latest DHT and relay readings for the title bar, from net_latency. Null until the first probe.
  netLatency: NetLatency | null;
  dismissUpdate: () => void;
  startFirstRun: () => void;
  finishFirstRun: () => Promise<boolean>;
  setDisplayName: (name: string) => Promise<boolean>;
  setAvatar: (bytes: string) => Promise<boolean>;
  clearAvatar: () => Promise<boolean>;
  restoreIdentity: (phrase: string) => Promise<boolean>;
  setNetworkSettings: (settings: NetworkSettings) => Promise<NetworkSettings | null>;
}

// Runs a user-triggered action: a failure becomes a toast and the action resolves without throwing.
async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn();
  } catch (err) {
    toast(err);
  }
}

const normalizeStatus = (s: Partial<UserStatus> | null | undefined): UserStatus => ({
  status: s?.status ?? 'online',
  until_ms: s?.until_ms ?? null,
});

// Shared by every path that sets the call: the voice connection lasts only while the call is a channel call here.
function withCall(state: ShellState, next: CallState | null): Partial<ShellState> {
  return {
    call: next,
    voiceConnection: next?.scope === 'channel' ? state.voiceConnection : null,
  };
}

export const useShell = create<ShellState>()((set, get) => ({
  me: null,
  status: { status: 'online', until_ms: null },
  version: '',
  communities: [],
  conversations: [],
  pendingDeviceLink: null,
  netLatency: null,
  route: { kind: 'home' },
  community: null,
  channelId: null,
  voiceChannelId: null,
  peer: null,
  peerName: '',
  group: null,
  page: null,
  unread: new Set<string>(),
  back: [],
  forward: [],
  contacts: null,
  ignored: [],
  transfers: [],
  transferRates: null,
  call: null,
  videoFrames: {},
  voiceConnection: null,
  update: null,
  updateDismissed: false,
  settings: null,
  onboarding: false,
  titleOverride: null,
  typing: {},
  ready: false,

  boot: async () => {
    set({
      route: { kind: 'home' },
      community: null,
      channelId: null,
      peer: null,
      group: null,
      peerName: '',
      page: has('contacts') ? 'contacts' : null,
    });
    set({ me: await api.me() });
    await get().loadCommunities();
    await get().loadConversations();
    await get().refreshUnread();
    await Promise.all([get().loadContacts(), get().loadIgnored(), get().loadTransfers(), get().loadCallState()]);
    set({ ready: true });
  },

  loadStatus: async () => {
    if (!has('status')) return;
    try {
      set({ status: normalizeStatus(await api.getStatus()) });
    } catch (err) {
      toast(err);
    }
  },

  loadCommunities: async () => {
    const communities = await api.listCommunities();
    set({ communities });
    const { route } = get();
    if (route.kind === 'server' && !communities.some((c) => c.id === route.communityId)) await get().openHome();
  },

  // The rail's order moves at once; it is saved on this device, and a failed save puts the old order back.
  moveServer: async (communityId, index) => {
    const before = get().communities;
    const from = before.findIndex((c) => c.id === communityId);
    if (from < 0 || index < 0 || index >= before.length || index === from) return;
    const next = [...before];
    const [moved] = next.splice(from, 1);
    next.splice(index, 0, moved);
    set({ communities: next });
    try {
      await api.setServerOrder(next.map((c) => c.id));
    } catch (err) {
      set({ communities: before });
      toast(err);
    }
  },

  loadConversations: async () => {
    const [listed, pins] = await Promise.all([
      api.listConversations(),
      has('pins') ? api.pinnedConversations().catch(() => [] as string[]) : Promise.resolve([] as string[]),
    ]);
    // The engine keeps pins as conversation keys; pinned conversations go first, in their listed order.
    const pinned = new Set(pins);
    const marked: ConversationSummary[] = listed.map((c) => ({ ...c, pinned: pinned.has(convKey(c)) }));
    const conversations = [...marked.filter((c) => c.pinned), ...marked.filter((c) => !c.pinned)];
    const { group } = get();
    set({
      conversations,
      group: group ? (conversations.find((c): c is GroupSummary => c.kind === 'group' && c.id === group.id) ?? group) : null,
    });
  },

  loadCommunity: async (communityId) => {
    const view = await api.communityView(communityId);
    const { route, channelId, voiceChannelId } = get();
    // A slow reply for a server the user has left is dropped.
    if (route.kind !== 'server' || route.communityId !== communityId) return;
    // The open text channel stays; otherwise the server opens on #general, or its first text channel.
    const texts = view.channels.filter((c) => !c.voice);
    const keepVoice = voiceChannelId !== null && view.channels.some((c) => c.id === voiceChannelId && c.voice);
    const nextChannel = !keepVoice && !texts.some((c) => c.id === channelId)
      ? (texts.find((c) => c.name === 'general') ?? texts[0])?.id ?? null
      : channelId;
    set({ community: view, channelId: nextChannel, voiceChannelId: keepVoice ? voiceChannelId : null });
  },

  loadContacts: async () => {
    if (!has('contacts')) return;
    try {
      set({ contacts: (await api.listContacts()) as ContactsView });
    } catch {
      set({ contacts: null });
    }
  },

  loadIgnored: async () => {
    if (!has('ignore')) return;
    try {
      set({ ignored: await api.ignoredUsers() });
    } catch {
      set({ ignored: [] });
    }
  },

  loadTransfers: async () => {
    if (!has('transfers')) return;
    try {
      const result = await api.listTransfers();
      const list = Array.isArray(result) ? result : (result.transfers ?? []);
      set({
        transfers: list,
        transferRates: Array.isArray(result) ? null : ((result.rates as ShellState['transferRates']) ?? null),
      });
    } catch {
      set({ transfers: [] });
    }
  },

  loadCallState: async () => {
    if (!has('calls')) return;
    let next: CallState | null = null;
    try {
      next = await api.callState();
    } catch {
      next = null;
    }
    set((s) => withCall(s, next));
  },

  // Checks every channel and conversation against its last-seen marker. A marker that does not exist yet is set to the
  // current newest message, so an existing install does not start with everything unread.
  refreshUnread: async () => {
    const { communities, route, channelId, community } = get();
    const openKey = route.kind === 'server' && community && channelId ? channelKey(community.id, channelId) : null;
    const unread = new Set(get().unread);
    for (const c of communities) {
      let view: CommunityView;
      try {
        view = await api.communityView(c.id);
      } catch {
        continue;
      }
      for (const ch of view.channels.filter((x) => !x.voice)) {
        const key = channelKey(c.id, ch.id);
        if (key === openKey) continue;
        let latest: { id: string; ms: number | string } | undefined;
        try {
          latest = (await api.channelMessages(c.id, ch.id, 1))[0] as typeof latest;
        } catch {
          continue;
        }
        if (!latest) continue;
        const seen = seenMs(key);
        if (seen === null) markSeen(key, latest.id, Number(latest.ms));
        else if (Number(latest.ms) > seen) unread.add(key);
      }
    }
    const openPeer = route.kind === 'home' ? get().peer : null;
    for (const conv of get().conversations) {
      const key = convKey(conv);
      if ((conv.kind === 'group' ? conv.id : conv.peer) === openPeer) continue;
      const seen = seenMs(key);
      if (conv.last_ms === undefined) continue;
      if (seen === null) markSeen(key, conv.kind === 'group' ? conv.id : conv.peer, Number(conv.last_ms));
      else if (Number(conv.last_ms) > seen) unread.add(key);
    }
    set({ unread });
  },

  openHome: () =>
    navigate(() =>
      guard(async () => {
        set({
          route: { kind: 'home' },
          community: null,
          channelId: null,
          voiceChannelId: null,
          peer: null,
          peerName: '',
          group: null,
          page: has('contacts') ? 'contacts' : null,
        });
        await Promise.all([get().loadConversations(), has('contacts') ? get().loadContacts() : null]);
      }),
    ),

  openServer: (communityId, channelId = null) =>
    navigate(() =>
      guard(async () => {
        set({
          route: { kind: 'server', communityId },
          peer: null,
          peerName: '',
          group: null,
          page: null,
          community: null,
          channelId,
          voiceChannelId: null,
        });
        await get().loadCommunity(communityId);
      }),
    ),

  openChannel: (channelId) =>
    navigate(() =>
      guard(async () => {
        set({ channelId, voiceChannelId: null, page: null });
      }),
    ),

  openPage: (page) =>
    navigate(() =>
      guard(async () => {
        if (page === 'contacts' && !has('contacts')) return;
        if (page === 'transfers' && !has('transfers')) return;
        const { route, peer, group } = get();
        if (route.kind !== 'home' || peer || group) {
          set({ route: { kind: 'home' }, peer: null, peerName: '', group: null });
        }
        set({ page });
        if (page === 'contacts') await get().loadContacts();
        if (page === 'transfers') await get().loadTransfers();
      }),
    ),

  openDm: (peer, name) =>
    navigate(() =>
      guard(async () => {
        // A conversation that Close DM hid comes back into the list when it is opened again.
        if (!get().conversations.some((c) => convKey(c) === `dm:${peer}`)) {
          await api.reopenConversation(`dm:${peer}`);
          await get().loadConversations();
        }
        set({ route: { kind: 'home' }, page: null, group: null, peer, peerName: name, channelId: null });
      }),
    ),

  clearDmState: (peer) =>
    set((s) => {
      const unread = new Set(s.unread);
      unread.delete(`dm:${peer}`);
      const typing = { ...s.typing };
      delete typing[typingKey({ kind: 'dm', peer })];
      return { unread, typing };
    }),

  openGroup: (conv) =>
    navigate(() =>
      guard(async () => {
        set({ route: { kind: 'home' }, page: null, group: conv, peer: conv.id, peerName: conv.name, channelId: null });
      }),
    ),

  openConversation: async (conv) => {
    if (conv.kind === 'group') await get().openGroup(conv);
    else await get().openDm(conv.peer, conv.display_name);
  },

  goBack: () => travel('back'),
  goForward: () => travel('forward'),

  setStatus: async (status, untilMs) => {
    await guard(async () => {
      await api.setStatus(status, untilMs);
      set({ status: { status, until_ms: untilMs } });
    });
  },

  toggleMuted: async () => {
    await guard(async () => {
      const { call } = get();
      if (!call) return;
      const next = !call.muted;
      await api.setMuted(next);
      set({ call: { ...call, muted: next } });
    });
  },

  toggleScreenShare: async (target) => {
    await guard(async () => {
      const { call } = get();
      if (!call) return;
      const next = target !== undefined || !call.screen;
      await api.setScreenShare(next, target);
      set({ call: { ...call, screen: next } });
    });
  },

  toggleCamera: async () => {
    await guard(async () => {
      const { call } = get();
      if (!call) return;
      const next = !call.camera;
      await api.setCamera(next);
      set({ call: { ...call, camera: next } });
    });
  },

  toggleDeafened: async () => {
    await guard(async () => {
      const { call } = get();
      if (!call) return;
      const next = !call.deafened;
      await api.setDeafened(next);
      set({ call: { ...call, deafened: next } });
    });
  },

  leaveCall: async () => {
    await guard(async () => {
      await api.leaveCall();
      set((s) => ({ ...withCall(s, null), voiceChannelId: null }));
    });
  },

  // Engine events. A message or conversation in the open place is read by its view; anything else is marked unread.
  handleEngineEvent: async (ev) => {
    await guard(async () => {
      switch (ev.kind) {
        case 'communities_changed':
          await get().loadCommunities();
          await get().refreshUnread();
          return;
        case 'community_updated': {
          const { route } = get();
          if (route.kind === 'server' && route.communityId === ev.community_id) await get().loadCommunity(ev.community_id);
          return;
        }
        case 'message': {
          const { route, community, channelId } = get();
          const open = route.kind === 'server' && community?.id === ev.community_id && channelId === ev.channel_id;
          if (!open) set({ unread: new Set([...get().unread, channelKey(ev.community_id, ev.channel_id)]) });
          return;
        }
        case 'dm': {
          await get().loadConversations();
          const { route, peer } = get();
          if (!(route.kind === 'home' && peer === ev.peer)) set({ unread: new Set([...get().unread, `dm:${ev.peer}`]) });
          return;
        }
        case 'group': {
          await get().loadConversations();
          const { route, group } = get();
          if (!(route.kind === 'home' && group?.id === ev.group_id)) set({ unread: new Set([...get().unread, `group:${ev.group_id}`]) });
          return;
        }
        case 'video_frame': {
          const key = ev.screen ? `${ev.root_id}:screen` : ev.root_id;
          set({ videoFrames: { ...get().videoFrames, [key]: { width: ev.width, height: ev.height, jpeg: ev.jpeg } } });
          return;
        }
        case 'net_latency':
          set({ netLatency: { dht_ms: ev.dht_ms, relay_ms: ev.relay_ms } });
          return;
        case 'contacts':
          await get().loadContacts();
          return;
        case 'presence': {
          await get().loadConversations();
          const { route, community } = get();
          if (route.kind === 'server' && community) await get().loadCommunity(community.id);
          return;
        }
        default:
          return;
      }
    });
  },

  handleCallEvent: (call) => {
    if (!has('calls')) return;
    set((s) => withCall(s, call));
  },

  handleTransfersEvent: (payload) => {
    if (!has('transfers')) return;
    if (Array.isArray(payload)) {
      set({ transfers: payload });
    } else if (payload && Array.isArray(payload.transfers)) {
      set((s) => ({ transfers: payload.transfers ?? [], transferRates: payload.rates ?? s.transferRates }));
    }
  },

  applyStatus: (payload) => {
    set({ status: normalizeStatus(payload) });
  },

  applyTyping: ({ target, names }) => {
    set((s) => ({ typing: { ...s.typing, [typingKey(target)]: names } }));
  },

  setUpdate: (info) => {
    set({ update: info, updateDismissed: false });
  },

  openSettings: (section = 'account') => {
    set({ settings: section });
  },

  closeSettings: () => {
    set({ settings: null });
  },

  dismissUpdate: () => {
    set({ updateDismissed: true });
  },

  // The first-run identity screen: shown once when the engine reports `me.first_run` (and the feature is there).
  startFirstRun: () => {
    if (!has('onboarding') || !get().me?.first_run) return;
    set({ onboarding: true, titleOverride: 'Welcome to Dissent' });
  },

  // The engine clears the flag; the screen only hides once that has happened.
  finishFirstRun: async () => {
    let done = false;
    await guard(async () => {
      await api.completeFirstRun();
      set({ onboarding: false, titleOverride: null });
      done = true;
    });
    return done;
  },

  setDisplayName: async (name) => {
    let saved = false;
    await guard(async () => {
      await api.setDisplayName(name);
      set({ me: await api.me() });
      toast('Display name saved', 'info');
      saved = true;
    });
    return saved;
  },

  // Profile picture: the engine's responses carry the new picture (see lib/images.ts), so the views only redraw.
  setAvatar: async (bytes) => {
    let saved = false;
    await guard(async () => {
      await api.setAvatar(bytes);
      set({ me: await api.me() });
      saved = true;
    });
    return saved;
  },

  clearAvatar: async () => {
    let saved = false;
    await guard(async () => {
      await api.clearAvatar();
      set({ me: await api.me() });
      saved = true;
    });
    return saved;
  },

  // Replaces this device's identity with the one from the phrase, then reloads the shell from it.
  restoreIdentity: async (phrase) => {
    let restored = false;
    await guard(async () => {
      await api.identityImportPhrase(phrase);
      await get().boot();
      toast('Identity restored', 'info');
      restored = true;
    });
    return restored;
  },

  // Saves relays and restarts the engine. The community and conversation lists are reloaded after the restart.
  setNetworkSettings: async (settings) => {
    let saved: NetworkSettings | null = null;
    await guard(async () => {
      saved = await api.setNetworkSettings(settings);
      await get().loadCommunities();
      await get().loadConversations();
      toast('Network settings saved. The engine restarted.', 'info');
    });
    return saved;
  },

  // A changed picture: a server's when the event names one, otherwise your own and the lists that show people.
  avatarChanged: async (payload) => {
    await guard(async () => {
      if (payload?.community_id) {
        const { route } = get();
        if (route.kind === 'server' && route.communityId === payload.community_id) await get().loadCommunity(payload.community_id);
        await get().loadCommunities();
        return;
      }
      set({ me: await api.me() });
      await Promise.all([get().loadConversations(), get().loadContacts()]);
      const { route, community } = get();
      if (route.kind === 'server' && community) await get().loadCommunity(community.id);
    });
  },
}));
