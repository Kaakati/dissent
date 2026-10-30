// Desktop notifications for new messages, shown by the native notification plugin while the window is minimized or
// unfocused. A click restores the window and opens the conversation (see alerts.ts openNotifiedConversation). Whether a
// message alerts at all is decided by alerts.ts shouldAlert, shared with the notification sound. Ported from the old UI's osnotify.js.
import { markdownPlain } from './markdown';
import { useShellPrefs } from './shell-prefs';
import { useSoundSettings } from './sound-settings';
import { currentWindow } from './tauri';
import { pageFocused } from './viewing';

const BODY_MAX = 140; // characters
const GAP_MS = 5000; // one notification per conversation every 5 s: a burst shows its first message only
const lastShown = new Map<string, number>(); // conversation key -> time of the last notification

// The window counts as in the background when it is minimized or hidden, or neither it nor the page has focus. The page's
// focus counts on its own: on Windows the native window reports no focus while the webview inside it has it, which used
// to notify (and badge the taskbar) for messages the user was reading.
async function windowInBackground(): Promise<boolean> {
  const win = currentWindow();
  if (!win) return !pageFocused();
  try {
    const [minimized, focused] = await Promise.all([win.isMinimized(), win.isFocused()]);
    return minimized || document.visibilityState === 'hidden' || !(focused || pageFocused());
  } catch {
    return !pageFocused();
  }
}

export function truncate(text: string, max = BODY_MAX): string {
  const chars = Array.from(String(text ?? '').trim());
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : chars.join('');
}

type Content = 'both' | 'sender' | 'none';

// What a message notification says for the "Show in notifications" setting. Lock-screen safe below 'both': no text,
// and with 'none' not even who.
export function messageNotice(content: Content, who: string, text: string): { title: string; body: string } {
  if (content === 'none') return { title: 'Dissent', body: 'New message' };
  return { title: who, body: content === 'sender' ? 'New message' : truncate(markdownPlain(text)) };
}

// The same for a reminder: the message's text only with 'both', its author with 'sender', and just "Reminder" with 'none'.
export function reminderNotice(content: Content, author: string, text: string): { title: string; body: string } {
  if (content === 'none') return { title: 'Dissent', body: 'Reminder' };
  if (content === 'sender') return { title: 'Reminder', body: `From ${author}` };
  return { title: 'Reminder', body: `${author} — ${truncate(markdownPlain(text), 120)}` };
}

// `where` is the group name or #channel, empty for a DM.
export async function showOsNotification({ key, sender, where, text }: { key: string; sender: string; where: string; text: string }): Promise<void> {
  if (useShellPrefs.getState().notificationsMuted) return;
  if (!(await windowInBackground())) return;
  const now = Date.now();
  if (now - (lastShown.get(key) ?? 0) < GAP_MS) return;
  lastShown.set(key, now);
  const who = where ? `${sender} · ${where}` : sender;
  const options = messageNotice(useSoundSettings.getState().content, who, text);
  try {
    await window.__TAURI__?.core.invoke('plugin:notification|notify', { options: { ...options, extra: { conv: key } } });
  } catch {
    // The OS may refuse. The sound and the unread marker still show the message.
  }
}

// A reminder's time came. It shows whatever the focus, since the viewer asked for it, but follows the desktop
// notifications switch and "Show in notifications" like a message does. A click arrives as `reminder:<id>` (see
// lib/events.ts), and the reminder's message opens.
export async function notifyReminder(id: string, author: string, text: string): Promise<void> {
  if (useShellPrefs.getState().notificationsMuted) return;
  const options = reminderNotice(useSoundSettings.getState().content, author, text);
  try {
    await window.__TAURI__?.core.invoke('plugin:notification|notify', { options: { ...options, extra: { conv: `reminder:${id}` } } });
  } catch {
    // The OS may refuse. The toast still shows the reminder.
  }
}
