// Opens the conversation a saved, linked or reminded message is in, and scrolls to it. Used by the Saved Messages view,
// a message link, and a reminder (its toast and its notification). A place the viewer has no conversation for, or a
// message that is not in the loaded history (the newest 200 of a conversation), is a toast and no move.
import { useChat } from '@/lib/chat';
import { notifyReminder, truncate } from '@/lib/osnotify';
import { channelKey, convKey, useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { MessageRef, Reminder } from '@/lib/types';
import { jumpTo } from './parts';

const LOAD_WAIT_MS = 4000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Opens the message's place in the shell. Resolves to the conversation's key, or null when there is no such place.
async function openPlace(ref: MessageRef): Promise<string | null> {
  const shell = useShell.getState();
  if (ref.community && ref.channel) {
    if (!shell.communities.some((c) => c.id === ref.community)) return null;
    await shell.openServer(ref.community, ref.channel);
    return channelKey(ref.community, ref.channel);
  }
  if (!ref.peer) return null;
  await shell.loadConversations();
  const conv = useShell.getState().conversations.find((c) => (c.kind === 'group' ? c.id : c.peer) === ref.peer);
  if (!conv) return null;
  await useShell.getState().openConversation(conv);
  return convKey(conv);
}

// Waits until the open conversation has loaded and holds the message.
async function loaded(key: string, id: string): Promise<boolean> {
  const until = Date.now() + LOAD_WAIT_MS;
  while (Date.now() < until) {
    const s = useChat.getState();
    if (s.target?.key === key && !s.loading && [...s.messages, ...s.dmMessages].some((m) => m.id === id)) return true;
    await sleep(100);
  }
  return false;
}

// Opens the message and scrolls to it. When it cannot, `missing` is the toast and the result is false.
export async function openMessage(ref: MessageRef, missing: string): Promise<boolean> {
  const key = await openPlace(ref).catch(() => null);
  if (!key || !(await loaded(key, ref.message_id))) {
    toast(missing, 'info');
    return false;
  }
  await sleep(50); // the list renders the row after the store changes
  jumpTo(ref.message_id);
  return true;
}

// Reminders that fired this session, by id. A notification click names the reminder, and this finds its message.
const fired = new Map<string, MessageRef>();

// A reminder's time came: a toast with a Jump button, and a desktop notification. Either opens the message.
export function reminderFired(reminder: Reminder): void {
  const { message } = reminder;
  fired.set(reminder.id, message);
  const author = message.author_name || 'unknown';
  const text = `Reminder: ${author} — ${truncate(message.text, 120)}`;
  toast(text, 'info', { label: 'Jump', run: () => void openReminderMessage(reminder.id) });
  void notifyReminder(reminder.id, author, message.text);
}

export async function openReminderMessage(id: string): Promise<void> {
  const message = fired.get(id);
  if (message) await openMessage(message, "You don't have access to this message");
}
