// The open conversation's messages and the composer's state. Ported from the message parts of the old UI's state.js and
// the old UI's actions.js (loadMessages, loadDm, openChannel, openDm, the message engine events, attachment progress).
// The shell store says which conversation is open; this store follows it. A change of the open conversation loads its
// messages (see startChat), so the shell's navigation actions stay as they are.
import { create } from 'zustand';
import { has } from './features';
import { api, listenTo } from './api';
import { baseName, guessMime } from './attachments';
import { markSeen, seenMs } from './unread';
import { toast } from './toasts';
import { openedTarget } from './target';
import { typingTarget } from './typing';
import { convKey, typingKey, useShell } from './store';
import type {
  Attachment,
  ChannelMessage,
  ChatTarget,
  ConversationMessage,
  EngineEvent,
  Id,
} from './types';

export const MSG_LIMIT = 200;
const PENDING_RETRY_MS = 5000; // pending DMs are retried by the engine outbox, so the open one is read again

export interface StagedFile {
  path: string;
  name: string;
  size: number | null;
  mime: string;
}

// The parts of the peer's profile the header and the welcome read: the safety state and the mutual servers.
export interface PeerProfile {
  suffix?: string;
  verified_ms?: number | null;
  key_changed_ms?: number | null;
  mutual_communities?: unknown[];
}

// Confirmations of the age gate, kept on this device. Storage can be unavailable: the session set still lets it open.
export const ageKey = (communityId: Id, channelId: Id): string => `age-ok:${communityId}:${channelId}`;
const ageRemembered = (key: string): boolean => {
  try {
    return window.localStorage.getItem(key) === 'yes';
  } catch {
    return false;
  }
};
export const rememberAge = (key: string): void => {
  try {
    window.localStorage.setItem(key, 'yes');
  } catch {
    // The session set in the store still lets the channel open.
  }
};
export const ageConfirmed = (key: string, sessionSet: ReadonlySet<string>): boolean => sessionSet.has(key) || ageRemembered(key);

interface ChatState {
  target: ChatTarget | null; // the open conversation
  messages: ChannelMessage[]; // a channel's messages, oldest first
  dmMessages: ConversationMessage[]; // a direct conversation's or a group's messages, oldest first
  historyStart: boolean; // the loaded history reaches the first message
  loading: boolean; // skeleton rows while the conversation loads
  newFromId: string | null; // first message after the last-seen point, for the divider
  peerProfile: PeerProfile | null; // the open direct conversation's peer
  filter: string; // in-chat search, applied to the loaded messages
  replyTo: { id: Id; authorName: string } | null; // a reply in progress
  editingId: Id | null; // the message being edited in place
  staged: StagedFile[]; // files waiting in the composer
  dropping: boolean; // files are dragged over the window
  revealed: ReadonlySet<string>; // spoiler messages the viewer has revealed
  revealedIgnored: ReadonlySet<string>; // messages of ignored people the viewer chose to show
  ageSession: ReadonlySet<string>; // age-gate confirmations made in this session

  setFilter: (text: string) => void;
  startReply: (id: Id, authorName: string) => void;
  cancelReply: () => void;
  beginEdit: (id: Id | null) => void;
  stageFiles: (files: StagedFile[]) => void;
  unstage: (path: string) => void;
  clearStaged: () => void;
  setDropping: (on: boolean) => void;
  reveal: (id: string) => void;
  revealIgnored: (id: string) => void;
  confirmAge: (key: string) => void;
}

// The timer for a direct conversation with pending messages, and a counter so a slow load for a conversation the user
// has left does not overwrite the one on screen.
let pendingTimer: ReturnType<typeof setTimeout> | null = null;
let generation = 0;

export const useChat = create<ChatState>()((set, get) => ({
  target: null,
  messages: [],
  dmMessages: [],
  historyStart: false,
  loading: false,
  newFromId: null,
  peerProfile: null,
  filter: '',
  replyTo: null,
  editingId: null,
  staged: [],
  dropping: false,
  revealed: new Set<string>(),
  revealedIgnored: new Set<string>(),
  ageSession: new Set<string>(),

  setFilter: (text) => set({ filter: text }),
  startReply: (id, authorName) => set({ replyTo: { id, authorName } }),
  cancelReply: () => set({ replyTo: null }),
  beginEdit: (id) => set({ editingId: id }),
  stageFiles: (files) => {
    const known = new Set(get().staged.map((f) => f.path));
    set({ staged: [...get().staged, ...files.filter((f) => f.path && !known.has(f.path))] });
  },
  unstage: (path) => set({ staged: get().staged.filter((f) => f.path !== path) }),
  clearStaged: () => set({ staged: [] }),
  setDropping: (on) => set({ dropping: on }),
  reveal: (id) => set({ revealed: new Set([...get().revealed, id]) }),
  revealIgnored: (id) => set({ revealedIgnored: new Set([...get().revealedIgnored, id]) }),
  confirmAge: (key) => {
    set({ ageSession: new Set([...get().ageSession, key]) });
    rememberAge(key);
  },
}));

// Same conversation: a channel by its ids, a direct or group conversation by its key.
const sameTarget = (a: ChatTarget | null, b: ChatTarget | null): boolean => (a?.key ?? null) === (b?.key ?? null);

// The conversation's messages from the engine, oldest first.
async function readTarget(target: ChatTarget): Promise<ChannelMessage[] | ConversationMessage[]> {
  switch (target.kind) {
    case 'channel': return (await api.channelMessages(target.communityId, target.channelId, MSG_LIMIT)) as ChannelMessage[];
    case 'dm': return (await api.dmMessages(target.peer, MSG_LIMIT)) as ConversationMessage[];
    case 'group': return (await api.groupMessages(target.groupId, MSG_LIMIT)) as ConversationMessage[];
  }
}

// Puts the items into the store, unless the user has moved to another conversation meanwhile.
function apply(target: ChatTarget, items: ChannelMessage[] | ConversationMessage[]): boolean {
  if (!sameTarget(useChat.getState().target, target)) return false;
  if (target.kind === 'channel') useChat.setState({ messages: items as ChannelMessage[] });
  else useChat.setState({ dmMessages: items as ConversationMessage[] });
  useChat.setState({ historyStart: items.length < MSG_LIMIT });
  return true;
}

// A direct conversation whose newest messages are still pending is read again, as the engine delivers them.
function schedulePending(target: ChatTarget, items: { pending?: boolean }[]): void {
  clearTimeout(pendingTimer ?? undefined);
  pendingTimer = null;
  if (target.kind === 'channel' || !items.some((m) => m.pending)) return;
  pendingTimer = setTimeout(() => void reloadOpen(), PENDING_RETRY_MS);
}

// Marks the conversation read up to its newest message, and clears its unread marker.
function markLatestRead(key: string, items: { id: string; ms: number }[]): void {
  const last = items[items.length - 1];
  if (!last) return;
  markSeen(key, last.id, Number(last.ms));
  // A conversation with unread messages is read in the engine too, so its count (and the rail's) clears.
  const counted = key.startsWith('dm:') || key.startsWith('group:');
  if (counted && useShell.getState().conversations.some((c) => convKey(c) === key && (c.unread ?? 0) > 0)) {
    void api.markRead(key).then(() => useShell.getState().loadConversations()).catch(() => {});
  }
  useShell.setState((s) => {
    if (!s.unread.has(key)) return {};
    const unread = new Set(s.unread);
    unread.delete(key);
    return { unread };
  });
}

async function loadPeerProfile(peer: Id): Promise<void> {
  try {
    const view = (await api.userProfile(peer)) as PeerProfile;
    if (useChat.getState().target?.key === `dm:${peer}`) useChat.setState({ peerProfile: view });
  } catch {
    useChat.setState({ peerProfile: null });
  }
}

// Opens a conversation: its messages load, the divider goes before the first message after the last-seen point, and
// the conversation is marked read.
function enter(next: ChatTarget | null): void {
  generation += 1;
  const seq = generation;
  clearTimeout(pendingTimer ?? undefined);
  pendingTimer = null;
  if (!next) {
    useChat.setState({
      target: null, messages: [], dmMessages: [], historyStart: false, loading: false, newFromId: null,
      peerProfile: null, filter: '', replyTo: null, editingId: null, staged: [],
    });
    return;
  }
  const seenBefore = seenMs(next.key);
  useChat.setState({
    target: next, messages: [], dmMessages: [], historyStart: false, loading: true, newFromId: null,
    peerProfile: null, filter: '', replyTo: null, editingId: null, staged: [],
  });
  if (next.kind === 'dm') void loadPeerProfile(next.peer);
  // Opening a channel clears its mention count, as opening it clears the unread dot.
  if (next.kind === 'channel') void clearMentions(next.communityId, next.channelId);
  // The names typing here are read once; the engine then sends each change (typing-changed).
  const shell = useShell.getState();
  const engineTarget = typingTarget(next);
  if (!shell.typing[typingKey(engineTarget)]) {
    api.typingIn(engineTarget)
      .then((names) => {
        if (sameTarget(useChat.getState().target, next)) shell.applyTyping({ target: engineTarget, names });
      })
      .catch(() => {});
  }
  void (async () => {
    let items: ChannelMessage[] | ConversationMessage[] = [];
    try {
      items = await readTarget(next);
    } catch (err) {
      toast(err);
    }
    if (seq !== generation) return;
    apply(next, items);
    // The divider is for server channels only, as the current UI has it: a conversation's messages carry no divider.
    const newFromId = seenBefore === null || next.kind !== 'channel'
      ? null
      : (items as { id: string; ms: number }[]).find((m) => Number(m.ms) > seenBefore)?.id ?? null;
    useChat.setState({ loading: false, newFromId });
    markLatestRead(next.key, items as { id: string; ms: number }[]);
    schedulePending(next, items as { pending?: boolean }[]);
  })();
}

// Re-reads the open conversation (after a send, an edit, a reaction, or an engine event for it).
export async function reloadOpen(): Promise<void> {
  const target = useChat.getState().target;
  if (!target) return;
  const items = await readTarget(target);
  if (!apply(target, items)) return;
  schedulePending(target, items as { pending?: boolean }[]);
}

// Marks the open conversation read after an action of ours, so no "New messages" line appears above our own message.
export function markOpenRead(): void {
  const { target, messages, dmMessages } = useChat.getState();
  if (!target) return;
  markLatestRead(target.key, target.kind === 'channel' ? messages : dmMessages);
}

// Engine events for the open conversation. The shell store handles the lists and unread markers; this reads messages.
export async function reloadForEvent(ev: EngineEvent): Promise<void> {
  const { target } = useChat.getState();
  if (!target) return;
  if (ev.kind === 'attachment_changed') {
    // A preview arrived. The conversation is re-read, but nothing is marked read or alerted.
    await reloadOpen();
    return;
  }
  if (ev.kind === 'message' && target.kind === 'channel' && target.communityId === ev.community_id && target.channelId === ev.channel_id) {
    await reloadOpen();
    markOpenRead();
  }
  if (ev.kind === 'dm' && target.kind === 'dm' && target.peer === ev.peer) {
    await reloadOpen();
    markOpenRead();
  }
  if (ev.kind === 'group' && target.kind === 'group' && target.groupId === ev.group_id) {
    await reloadOpen();
    markOpenRead();
  }
}

// The attachment with this id in the open conversation, or null. The same object until the message changes.
export function findAttachment(id: Id): Attachment | null {
  const { messages, dmMessages } = useChat.getState();
  for (const m of [...messages, ...dmMessages]) {
    const found = m.attachments?.find((a) => a.id === id);
    if (found) return found;
  }
  return null;
}

// Upload progress updates the attachment in place; the next reload brings the final state.
export function attachmentProgress(p: { message_id: Id; attachment_id: Id; progress: number; rate_bps?: number; eta_s?: number; transfer?: unknown } | null): void {
  if (!p) return;
  const update = <T extends { id: string; attachments?: Attachment[] | null }>(items: T[]): T[] => {
    const at = items.findIndex((m) => m.id === p.message_id && m.attachments?.some((a) => a.id === p.attachment_id));
    if (at < 0) return items;
    const next = items.slice();
    const message = items[at];
    next[at] = {
      ...message,
      attachments: (message.attachments ?? []).map((a) => (a.id === p.attachment_id
        ? {
          ...a,
          state: 'downloading' as const,
          progress: Math.max(0, Math.min(1, Number(p.progress) || 0)),
          ...(p.rate_bps !== undefined ? { rate_bps: p.rate_bps } : {}),
          ...(p.eta_s !== undefined ? { eta_s: p.eta_s } : {}),
          ...(p.transfer !== undefined ? { transfer: p.transfer } : {}),
        }
        : a)),
    };
    return next;
  };
  const { messages, dmMessages } = useChat.getState();
  const nextMessages = update(messages);
  const nextDm = update(dmMessages);
  if (nextMessages !== messages || nextDm !== dmMessages) useChat.setState({ messages: nextMessages, dmMessages: nextDm });
}

// Whether files dropped or picked now would be sent: a conversation that takes attachments is open.
export function canAttachNow(): boolean {
  const s = useChat.getState();
  if (!s.target) return false;
  return s.target.kind === 'channel' ? s.messages.some((m) => m.attachments != null) : s.dmMessages.some((m) => m.attachments != null);
}

// Follows the shell: when the open conversation changes, the chat loads it. Call once at start-up (events.ts).
let started = false;
export function startChat(): void {
  if (started) return;
  started = true;
  listenTo('dissent://drag-over', () => {
    if (canAttachNow()) useChat.getState().setDropping(true);
  }).catch(() => {});
  listenTo('dissent://drag-leave', () => useChat.getState().setDropping(false)).catch(() => {});
  listenTo<{ path?: string; name?: string; size?: number | null; mime?: string }[]>('dissent://files-dropped', (files) => {
    useChat.getState().setDropping(false);
    if (!canAttachNow() || !Array.isArray(files)) return;
    useChat.getState().stageFiles(files.filter((f) => f && typeof f.path === 'string').map((f) => {
      const name = f.name ?? baseName(f.path ?? '');
      return { path: f.path ?? '', name, size: f.size ?? null, mime: f.mime ?? guessMime(name) };
    }));
  }).catch(() => {});
  useShell.subscribe((s) => {
    const next = openedTarget(s);
    if (sameTarget(next, useChat.getState().target)) return;
    enter(next);
  });
  // The shell may already hold a conversation (a restored place).
  enter(openedTarget(useShell.getState()));
}

// Marks a channel's mentions read and reloads the counts the badges read: the open server's channel list and the
// rail. Called when the channel opens, and when a message arrives in it while it is open and the window has focus.
export async function clearMentions(communityId: Id, channelId: Id): Promise<void> {
  if (!has('mentions')) return;
  try {
    await api.markMentionsRead(communityId, channelId);
  } catch {
    return;
  }
  const shell = useShell.getState();
  await Promise.all([
    shell.route.kind === 'server' && shell.route.communityId === communityId ? shell.loadCommunity(communityId) : Promise.resolve(),
    shell.loadCommunities(),
  ]).catch(() => {});
}
