// Notification and call sound settings, as a store so the settings page and the sound player read one value. Stored in
// localStorage (guarded). Ported from the settings half of sounds.js; the players themselves live in sounds.ts.
import { create } from 'zustand';

const KEY = 'dissent.sounds.v1';

// content: what a desktop notification shows: 'both' (sender and message), 'sender', or 'none' (just "New message").
// The default is 'sender': a notification shows on the lock screen and stays in the OS history, so the message text
// appears only when the user chose it. `contentChosen` records that choice; a saved 'both' from before the default
// changed (every save wrote the whole object) did not choose it, so it reads as the new default.
export interface SoundSettings {
  enabled: boolean; // Play sounds
  messages: boolean;
  calls: boolean;
  ringtone: boolean;
  volume: number; // 0 to 100
  content: 'both' | 'sender' | 'none';
  contentChosen: boolean;
}

const DEFAULTS: SoundSettings = { enabled: true, messages: true, calls: true, ringtone: true, volume: 80, content: 'sender', contentChosen: false };

function readSettings(): SoundSettings {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    const merged = { ...DEFAULTS, ...(saved && typeof saved === 'object' ? saved : {}) };
    const valid = ['both', 'sender', 'none'].includes(merged.content);
    // A saved 'none' is kept: it shows less than the default, so it was a choice either way.
    const content = !valid || (merged.content === 'both' && merged.contentChosen !== true) ? DEFAULTS.content : merged.content;
    return { ...merged, content, contentChosen: merged.contentChosen === true, volume: Math.min(100, Math.max(0, Number(merged.volume) || 0)) };
  } catch {
    return { ...DEFAULTS };
  }
}

export const useSoundSettings = create<SoundSettings>()(() => readSettings());

export function setSoundSettings(next: Partial<SoundSettings>): void {
  useSoundSettings.setState(next);
  try {
    window.localStorage.setItem(KEY, JSON.stringify(useSoundSettings.getState()));
  } catch {
    // Unavailable: the choice lasts for this session only.
  }
}
