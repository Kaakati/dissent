// Desktop notifications for new messages, shown by the native notification plugin while the window is minimized or
// unfocused. A click restores the window and opens the conversation (events.rs, then actions.openNotifiedConversation).
// Whether a message alerts at all is decided by actions.js shouldAlert, shared with the notification sound.
import { getSoundSettings } from './sounds.js';

const BODY_MAX = 140; // characters
const GAP_MS = 5000; // one notification per conversation every 5 s: a burst shows its first message only
const lastShown = new Map(); // conversation key -> time of the last notification

// The window counts as in the background when it is minimized or lacks focus. Without the window API (the browser
// preview), focus is the document's.
async function windowInBackground() {
  try {
    const win = window.__TAURI__?.window?.getCurrentWindow?.();
    if (!win) return !document.hasFocus();
    const [minimized, focused] = await Promise.all([win.isMinimized(), win.isFocused()]);
    return minimized || !focused;
  } catch {
    return !document.hasFocus();
  }
}

export function truncate(text, max = BODY_MAX) {
  const chars = Array.from(String(text ?? '').trim());
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : chars.join('');
}

// msg: { key, sender, where, text }. `where` is the group name or #channel, empty for a DM.
export async function showOsNotification({ key, sender, where, text }) {
  if (!(await windowInBackground())) return;
  const now = Date.now();
  if (now - (lastShown.get(key) ?? 0) < GAP_MS) return;
  lastShown.set(key, now);
  const { content } = getSoundSettings();
  const who = where ? `${sender} · ${where}` : sender;
  const options = content === 'none'
    ? { title: 'Dissent', body: 'New message' }
    : { title: who, body: content === 'sender' ? 'New message' : truncate(text) };
  try {
    await window.__TAURI__.core.invoke('plugin:notification|notify', { options: { ...options, extra: { conv: key } } });
  } catch {
    // The OS may refuse. The sound and the unread marker still show the message.
  }
}
