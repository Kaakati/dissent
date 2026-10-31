// New-message alerts: the notification sound and the desktop notification, decided by one rule. Ported from the alert
// half of the old UI's actions.js (shouldAlert, alertFor, alertForChannelMessage, alertForDm, alertForGroup) and the notification
// click (openNotifiedConversation).
import { has } from './features';
import { api } from './api';
import { isMuted } from './mutes';
import { showOsNotification } from './osnotify';
import { playNotification } from './sounds';
import { convKey, useShell } from './store';
import { viewing } from './viewing';
import type { ChannelMessage, DirectSummary, GroupSummary, Id } from './types';

interface AlertMessage {
  kind: 'channel' | 'dm' | 'group';
  key: string;
  id: Id; // the message's id: one message alerts once
  from: Id; // the author's root id
  sender: string;
  where: string; // "#channel" or the group's name, empty for a DM
  text: string;
  mentions?: boolean; // the message mentions the user
}

// One decision for the sound and the desktop notification. Muted, own, ignored and closed conversations never do, and
// neither does Do Not Disturb. A channel message alerts when the server's notification setting says so (the engine
// decides that before this runs), and a mention alerts through a mute, in a channel, a DM or a group alike.
function shouldAlert(msg: AlertMessage): boolean {
  const s = useShell.getState();
  const me = s.me;
  // A mention alerts even in a muted channel, the same as a DM; nothing alerts under Do Not Disturb.
  if (isMuted(msg.key) && !msg.mentions) return false;
  if (!me || msg.from === me.root_id || s.ignored.includes(msg.from)) return false;
  if (s.status.status === 'dnd') return false;
  if (msg.kind === 'channel') return true;
  return s.conversations.some((c) => convKey(c) === msg.key); // a closed conversation is no longer listed
}

// The messages already alerted on, oldest first. An event can name a message that already alerted (an edit, a
// reaction, or the same message announced again), and that must not sound or notify twice. Bounded, so the oldest
// id is forgotten past NOTIFIED_MAX.
const NOTIFIED_MAX = 500;
const notified = new Set<string>();

// True the first time a message alerts. Records it.
function firstAlert(key: string, id: Id): boolean {
  const mark = `${key}|${id}`;
  if (notified.has(mark)) return false;
  notified.add(mark);
  if (notified.size > NOTIFIED_MAX) notified.delete(notified.values().next().value as string);
  return true;
}

function alertFor(msg: AlertMessage): void {
  if (!shouldAlert(msg) || !firstAlert(msg.key, msg.id)) return;
  // A message the user watches arrive (see viewing.ts) needs no sound. The desktop notification shows only while the
  // window is minimized or unfocused (showOsNotification checks that), so it does not show for one either.
  if (!viewing(msg.key)) playNotification();
  void showOsNotification(msg);
}

// The engine event carries no text, so the newest message is read. Only a message that @-mentions the user alerts.
export async function alertForChannelMessage(ev: { community_id: Id; channel_id: Id }): Promise<void> {
  const [latest] = (await api.channelMessages(ev.community_id, ev.channel_id, 1)) as ChannelMessage[];
  const me = useShell.getState().me;
  if (!latest || !me) return;
  const mentioned = has('mentions') && latest.mentions_me === true;
  const byName = mentioned && (latest.mentions?.some((m) => m.id === me.root_id) ?? false);
  // Without the server_prefs capability only a mention alerts, as it did before server preferences existed.
  const alerts = has('server_prefs') ? await api.serverAlerts(ev.community_id, mentioned, byName) : mentioned;
  if (!alerts) return;
  const cached = useShell.getState().community;
  const community = cached?.id === ev.community_id ? cached : await api.communityView(ev.community_id);
  const channel = community.channels?.find((c) => c.id === ev.channel_id);
  alertFor({
    kind: 'channel',
    key: `ch:${ev.community_id}:${ev.channel_id}`,
    id: latest.id,
    from: latest.author,
    sender: latest.author_name,
    where: channel ? `#${channel.name}` : '',
    text: latest.text,
    mentions: mentioned,
  });
}

// Whether a direct or group message mentions the user, as the engine decided it.
const mentionsMe = (m: { mentions_me?: boolean }): boolean => has('dm_mentions') && m.mentions_me === true;

// Any DM message from the peer alerts. Call after the conversation list is reloaded, so a closed DM is not listed.
export async function alertForDm(peer: Id): Promise<void> {
  const [latest] = (await api.dmMessages(peer, 1)) as { id: Id; from: Id; text: string; mentions_me?: boolean }[];
  const conv = useShell.getState().conversations.find((c): c is DirectSummary => c.kind === 'dm' && c.peer === peer);
  if (!latest || !conv) return;
  alertFor({ kind: 'dm', key: `dm:${peer}`, id: latest.id, from: latest.from, sender: conv.display_name, where: '', text: latest.text, mentions: mentionsMe(latest) });
}

// Any group message alerts, and one that mentions the user alerts through a mute. Membership lines are not messages.
export async function alertForGroup(groupId: Id): Promise<void> {
  const [latest] = (await api.groupMessages(groupId, 1)) as { id: Id; from: Id; text: string; system?: boolean; mentions_me?: boolean }[];
  const conv = useShell.getState().conversations.find((c): c is GroupSummary => c.kind === 'group' && c.id === groupId);
  if (!latest || !conv || latest.system) return;
  const sender = conv.members?.find((m) => m.root_id === latest.from)?.display_name ?? 'Someone';
  alertFor({ kind: 'group', key: `group:${groupId}`, id: latest.id, from: latest.from, sender, where: conv.name, text: latest.text, mentions: mentionsMe(latest) });
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
