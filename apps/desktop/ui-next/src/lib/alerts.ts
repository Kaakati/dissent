// New-message alerts: the notification sound and the desktop notification, decided by one rule. Ported from the alert
// half of ui/actions.js (shouldAlert, alertFor, alertForChannelMessage, alertForDm, alertForGroup) and the notification
// click (openNotifiedConversation).
import { api } from './api';
import { isMuted } from './mutes';
import { showOsNotification } from './osnotify';
import { playNotification } from './sounds';
import { convKey, useShell } from './store';
import { openedTarget } from './target';
import type { ChannelMessage, DirectSummary, GroupSummary, Id } from './types';

interface AlertMessage {
  kind: 'channel' | 'dm' | 'group';
  key: string;
  from: Id; // the author's root id
  sender: string;
  where: string; // "#channel" or the group's name, empty for a DM
  text: string;
  mentions?: boolean; // channel messages only
}

const escapeRegExp = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

// True when the text @-mentions one of the names, as a whole word and ignoring case.
function mentions(text: string, names: (string | null | undefined)[]): boolean {
  return names.some((name) => name && new RegExp(`@${escapeRegExp(name)}(?![\\p{L}\\p{N}_])`, 'iu').test(text));
}

// One decision for the sound and the desktop notification. Muted, own, ignored, closed and channel messages without an
// @mention never do, and neither does Do Not Disturb.
function shouldAlert(msg: AlertMessage): boolean {
  const s = useShell.getState();
  const me = s.me;
  if (isMuted(msg.key)) return false;
  if (!me || msg.from === me.root_id || s.ignored.includes(msg.from)) return false;
  if (s.status.status === 'dnd') return false;
  if (msg.kind === 'channel') return Boolean(msg.mentions);
  return s.conversations.some((c) => convKey(c) === msg.key); // a closed conversation is no longer listed
}

// A new message in the conversation the user is reading, in a focused window, needs no sound. The desktop notification
// waits for the window to be minimized or unfocused (showOsNotification checks that).
function reading(key: string): boolean {
  const s = useShell.getState();
  return openedTarget(s)?.key === key && !s.page && document.hasFocus();
}

function alertFor(msg: AlertMessage): void {
  if (!shouldAlert(msg)) return;
  if (!reading(msg.key)) playNotification();
  void showOsNotification(msg);
}

// The engine event carries no text, so the newest message is read. Only a message that @-mentions the user alerts.
export async function alertForChannelMessage(ev: { community_id: Id; channel_id: Id }): Promise<void> {
  const [latest] = (await api.channelMessages(ev.community_id, ev.channel_id, 1)) as ChannelMessage[];
  const me = useShell.getState().me;
  if (!latest || !me || !latest.text?.includes('@')) return;
  const cached = useShell.getState().community;
  const community = cached?.id === ev.community_id ? cached : await api.communityView(ev.community_id);
  const handle = community.members?.find((m) => m.root_id === me.root_id)?.handle;
  const channel = community.channels?.find((c) => c.id === ev.channel_id);
  alertFor({
    kind: 'channel',
    key: `ch:${ev.community_id}:${ev.channel_id}`,
    from: latest.author,
    sender: latest.author_name,
    where: channel ? `#${channel.name}` : '',
    text: latest.text,
    mentions: mentions(latest.text, [me.display_name, handle]),
  });
}

// Any DM message from the peer alerts. Call after the conversation list is reloaded, so a closed DM is not listed.
export async function alertForDm(peer: Id): Promise<void> {
  const [latest] = (await api.dmMessages(peer, 1)) as { from: Id; text: string }[];
  const conv = useShell.getState().conversations.find((c): c is DirectSummary => c.kind === 'dm' && c.peer === peer);
  if (!latest || !conv) return;
  alertFor({ kind: 'dm', key: `dm:${peer}`, from: latest.from, sender: conv.display_name, where: '', text: latest.text });
}

// Any group message alerts. Membership lines in a group are not messages.
export async function alertForGroup(groupId: Id): Promise<void> {
  const [latest] = (await api.groupMessages(groupId, 1)) as { from: Id; text: string; system?: boolean }[];
  const conv = useShell.getState().conversations.find((c): c is GroupSummary => c.kind === 'group' && c.id === groupId);
  if (!latest || !conv || latest.system) return;
  const sender = conv.members?.find((m) => m.root_id === latest.from)?.display_name ?? 'Someone';
  alertFor({ kind: 'group', key: `group:${groupId}`, from: latest.from, sender, where: conv.name, text: latest.text });
}

// A desktop notification was clicked. The backend has restored the window; this opens the conversation it was for.
export async function openNotifiedConversation(key: unknown): Promise<void> {
  if (typeof key !== 'string') return;
  const [kind, a, b] = key.split(':');
  const shell = useShell.getState();
  if (kind === 'ch') return shell.openServer(a, b);
  await shell.loadConversations();
  const fresh = useShell.getState();
  if (kind === 'group') {
    const conv = fresh.conversations.find((c): c is GroupSummary => c.kind === 'group' && c.id === a);
    if (conv) await fresh.openGroup(conv);
    return;
  }
  const conv = fresh.conversations.find((c): c is DirectSummary => c.kind === 'dm' && c.peer === a);
  if (conv) await fresh.openDm(a, conv.display_name);
}
