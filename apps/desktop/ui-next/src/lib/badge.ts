// The unread badge on the taskbar button or dock icon (the app's badge.rs draws it and works out the count), and the
// message notifications the OS still holds. The badge follows the conversation list and the servers' mentions, and is
// sent again when the window gains or loses focus and when the open conversation changes. The conversation the user is
// viewing (see viewing.ts) never counts. Focusing the window clears the delivered notifications: on Windows they would
// otherwise keep a count on the taskbar button until the notification center is opened.
import { api } from './api';
import { catchUpOpen } from './chat';
import { isMuted } from './mutes';
import { convKey, useShell } from './store';
import { openedTarget } from './target';
import { pageFocused, viewing } from './viewing';

export interface BadgeSource {
  key: string;
  unread: number;
  mentions: number;
  muted: boolean;
}

// What is unread, for the badge: each direct or group conversation, and each server's mentions (a muted server's
// mentions still count, as on the rail). Entries with nothing unread are left out.
export function badgeSources(s: Pick<ReturnType<typeof useShell.getState>, 'conversations' | 'communities'>): BadgeSource[] {
  const sources: BadgeSource[] = [];
  for (const c of s.conversations) {
    const key = convKey(c);
    const unread = c.unread ?? 0;
    const mentions = c.mentions ?? 0;
    if (unread > 0 || mentions > 0) sources.push({ key, unread, mentions, muted: isMuted(key) });
  }
  for (const c of s.communities) {
    const mentions = c.mentions ?? 0;
    if (mentions > 0) sources.push({ key: `server:${c.id}`, unread: 0, mentions, muted: false });
  }
  return sources;
}

// The conversation on screen and being read, or null.
export function viewingKey(): string | null {
  const key = openedTarget(useShell.getState())?.key;
  return key && viewing(key) ? key : null;
}

let sent = '';

// Sends the badge when what it shows from has changed. Repeats are skipped, so this can run on every store change.
export function syncBadge(): void {
  const sources = badgeSources(useShell.getState());
  const watching = viewingKey();
  const next = JSON.stringify([sources, watching]);
  if (next === sent) return;
  sent = next;
  api.setUnreadBadge(sources, watching).catch(() => {});
}

// Coming back to the window reads what arrived in the open conversation meanwhile, if it is at its end.
function onFocusChange(): void {
  if (pageFocused()) {
    catchUpOpen();
    api.clearMessageNotifications().catch(() => {});
  }
  syncBadge();
}

let started = false;

// Idempotent: React's development double-mount must not subscribe twice.
export function startBadge(): void {
  if (started) return;
  started = true;
  useShell.subscribe(syncBadge);
  window.addEventListener('focus', onFocusChange);
  window.addEventListener('blur', onFocusChange);
  document.addEventListener('visibilitychange', onFocusChange);
  onFocusChange();
}
