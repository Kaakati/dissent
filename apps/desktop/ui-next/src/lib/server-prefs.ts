// This device's preferences for each server: a timed mute, which messages alert, whether @everyone counts and whether
// muted channels are hidden. The engine keeps them in a local file (server_prefs.bin) and never sends them anywhere. This
// store caches them per server, so the rail and the sidebar can read them without a round trip.
import { create } from 'zustand';
import { api } from './api';
import { toast } from './toasts';
import { clockLabel } from './time';
import type { Id, ServerPrefs } from './types';

// The mute that lasts until it is turned back on. Matches the engine's MUTED_FOREVER (2^63), which a JSON number carries.
export const MUTED_FOREVER = 2 ** 63;

export const DEFAULT_PREFS: ServerPrefs = { muted_until: null, notify: 'mentions', suppress_everyone: false, hide_muted_channels: false };

export const useServerPrefs = create<{ prefs: Record<Id, ServerPrefs> }>(() => ({ prefs: {} }));

export function prefsOf(communityId: Id): ServerPrefs {
  return useServerPrefs.getState().prefs[communityId] ?? DEFAULT_PREFS;
}

export function isServerMuted(prefs: ServerPrefs, now = Date.now()): boolean {
  return prefs.muted_until !== null && prefs.muted_until > now;
}

// The time a mute ends, as the subtitle shows it: "Muted until 2:30 PM", or until it is turned back on.
export function muteSubtitle(prefs: ServerPrefs): string {
  if (!isServerMuted(prefs)) return '';
  if (prefs.muted_until === MUTED_FOREVER) return 'Muted until I turn it back on';
  const time = clockLabel(prefs.muted_until ?? 0);
  return `Muted until ${time}`;
}

const NOTIFY_LABEL: Record<ServerPrefs['notify'], string> = {
  all: 'All Messages',
  mentions: 'Only @mentions',
  nothing: 'Nothing',
};

export function notifyLabel(prefs: ServerPrefs): string {
  return NOTIFY_LABEL[prefs.notify];
}

// A timed mute ends on its own: a timer re-reads the server's preferences when it does, so the engine drops the mute and
// the rail and the menus update without an interaction. Browsers cap a timer at about 24 days, which the next read re-arms.
const MAX_TIMER_MS = 2 ** 31 - 1;
const expiries = new Map<Id, ReturnType<typeof setTimeout>>();

function scheduleExpiry(communityId: Id, prefs: ServerPrefs): void {
  clearTimeout(expiries.get(communityId));
  expiries.delete(communityId);
  const until = prefs.muted_until;
  if (until === null || until >= MUTED_FOREVER) return;
  const delay = Math.min(Math.max(until - Date.now(), 0) + 50, MAX_TIMER_MS);
  expiries.set(
    communityId,
    setTimeout(() => {
      expiries.delete(communityId);
      void loadServerPrefs(communityId);
    }, delay),
  );
}

export async function loadServerPrefs(communityId: Id): Promise<void> {
  try {
    const prefs = (await api.getServerPrefs(communityId)) as ServerPrefs;
    useServerPrefs.setState((s) => ({ prefs: { ...s.prefs, [communityId]: prefs } }));
    scheduleExpiry(communityId, prefs);
  } catch {
    // Not loaded: the defaults apply until the next load.
  }
}

// Saves a change to one server's preferences. The cache changes at once, then reloads what the engine kept, which is the
// source of truth (a failed save shows the error and the engine's values).
export async function updateServerPrefs(communityId: Id, change: Partial<ServerPrefs>): Promise<void> {
  const next = { ...prefsOf(communityId), ...change };
  useServerPrefs.setState((s) => ({ prefs: { ...s.prefs, [communityId]: next } }));
  try {
    await api.setServerPrefs(communityId, next);
  } catch (err) {
    toast(err);
  }
  await loadServerPrefs(communityId);
}

// Mutes a server for a while, or until it is turned back on when `ms` is null.
export function muteServer(communityId: Id, ms: number | null): Promise<void> {
  return updateServerPrefs(communityId, { muted_until: ms === null ? MUTED_FOREVER : Date.now() + ms });
}

export function unmuteServer(communityId: Id): Promise<void> {
  return updateServerPrefs(communityId, { muted_until: null });
}
