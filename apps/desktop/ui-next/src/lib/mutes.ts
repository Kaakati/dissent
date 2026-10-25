// Muted conversations: a mute lasts until a time, or until it is turned off. Kept under the same localStorage key as
// ui/actions.js, so both UIs agree. A timed mute that has run out is removed the next time it is checked.
import { create } from 'zustand';

const MUTE_KEY = 'dissent.muted.v1';

// key -> the time the mute ends (epoch ms), or null for a mute that lasts until it is turned off.
type Mutes = Record<string, number | null>;

function load(): Mutes {
  try {
    const saved = JSON.parse(window.localStorage.getItem(MUTE_KEY) ?? '{}');
    return saved && typeof saved === 'object' ? saved : {};
  } catch {
    return {};
  }
}

function save(mutes: Mutes): void {
  try {
    window.localStorage.setItem(MUTE_KEY, JSON.stringify(mutes));
  } catch {
    // Unavailable: the mute lasts for this session only.
  }
}

export const useMutes = create<{ mutes: Mutes }>(() => ({ mutes: load() }));

export function isMuted(key: string): boolean {
  const { mutes } = useMutes.getState();
  if (!(key in mutes)) return false;
  const until = mutes[key];
  if (until === null || until > Date.now()) return true;
  const next = { ...mutes };
  delete next[key];
  save(next);
  useMutes.setState({ mutes: next });
  return false;
}

export function muteConversation(key: string, untilMs: number | null = null): void {
  const next = { ...useMutes.getState().mutes, [key]: untilMs };
  save(next);
  useMutes.setState({ mutes: next });
}

export function unmuteConversation(key: string): void {
  const next = { ...useMutes.getState().mutes };
  delete next[key];
  save(next);
  useMutes.setState({ mutes: next });
}
