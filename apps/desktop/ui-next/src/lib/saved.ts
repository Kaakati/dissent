// Saved messages (bookmarks), reminders and mark unread. The core keeps the first two on this device, and mark unread
// moves the engine's read marker. Each action reports a failure as a toast and resolves to false, as the other actions do.
import { api } from './api';
import { useShell } from './store';
import { toast } from './toasts';
import { stampLabel } from './time';
import type { Bookmark, ChatTarget, Id, MessageRef } from './types';

// The reference a message is saved and jumped to by: a channel, a direct conversation or a group (its id in `peer`).
export function refOf(target: ChatTarget, message: { id: Id; authorName: string; text: string; ms: number }): MessageRef {
  const place = target.kind === 'channel'
    ? { community: target.communityId, channel: target.channelId, peer: null }
    : { community: null, channel: null, peer: target.kind === 'dm' ? target.peer : target.groupId };
  return { ...place, message_id: message.id, text: message.text, author_name: message.authorName, ms: message.ms };
}

// The same message, by where it is and its id.
export function sameMessage(a: MessageRef, b: MessageRef): boolean {
  return a.message_id === b.message_id && (a.community ?? null) === (b.community ?? null) && (a.channel ?? null) === (b.channel ?? null) && (a.peer ?? null) === (b.peer ?? null);
}

export async function listSaved(): Promise<Bookmark[]> {
  try {
    return await api.listBookmarks();
  } catch (err) {
    toast(err);
    return [];
  }
}

// The saved copy of a message on this device, if there is one.
export async function savedCopy(ref: MessageRef): Promise<Bookmark | undefined> {
  return (await listSaved()).find((b) => sameMessage(b.message, ref));
}

export async function saveMessage(ref: MessageRef): Promise<boolean> {
  try {
    await api.addBookmark(ref);
  } catch (err) {
    toast(err);
    return false;
  }
  toast('Message saved', 'info');
  return true;
}

export async function unsaveMessage(key: string): Promise<boolean> {
  try {
    await api.removeBookmark(key);
  } catch (err) {
    toast(err);
    return false;
  }
  toast('Bookmark removed', 'info');
  return true;
}

export async function setReminder(ref: MessageRef, atMs: number): Promise<boolean> {
  try {
    await api.addReminder(ref, atMs);
  } catch (err) {
    toast(err);
    return false;
  }
  toast(`Reminder set for ${stampLabel(atMs)}`, 'info');
  return true;
}

// The engine moves the read marker just before the message, so it and later ones count as unread. The sidebar's dot is
// set here too, so it shows before the next refresh.
export async function markUnreadFrom(target: ChatTarget, ms: number): Promise<boolean> {
  try {
    await api.markUnread(target.key, ms);
  } catch (err) {
    toast(err);
    return false;
  }
  useShell.setState((s) => ({ unread: new Set([...s.unread, target.key]) }));
  toast('Marked unread', 'info');
  return true;
}

// The reminder times of the menu: 20 minutes, 1 hour and 3 hours from now, and 9:00 the next day (local time).
export function reminderChoices(now: Date = new Date()): { label: string; at: number }[] {
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  const minutes = (n: number) => now.getTime() + n * 60_000;
  return [
    { label: 'In 20 Minutes', at: minutes(20) },
    { label: 'In 1 Hour', at: minutes(60) },
    { label: 'In 3 Hours', at: minutes(180) },
    { label: 'Tomorrow', at: tomorrow.getTime() },
  ];
}
