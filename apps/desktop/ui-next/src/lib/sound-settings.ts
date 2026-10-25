// Notification and call sound settings, as a store so the settings page and the sound player read one value. Stored in
// localStorage (guarded). Ported from the settings half of sounds.js; the players themselves live in sounds.ts.
import { create } from 'zustand';

const KEY = 'dissent.sounds.v1';

// content: what a desktop notification shows: 'both' (sender and message), 'sender', or 'none' (just "New message").
export interface SoundSettings {
  enabled: boolean; // Play sounds
  messages: boolean;
  calls: boolean;
  ringtone: boolean;
  volume: number; // 0 to 100
  content: 'both' | 'sender' | 'none';
}

const DEFAULTS: SoundSettings = { enabled: true, messages: true, calls: true, ringtone: true, volume: 80, content: 'both' };

function readSettings(): SoundSettings {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    const merged = { ...DEFAULTS, ...(saved && typeof saved === 'object' ? saved : {}) };
    const content = ['both', 'sender', 'none'].includes(merged.content) ? merged.content : DEFAULTS.content;
    return { ...merged, content, volume: Math.min(100, Math.max(0, Number(merged.volume) || 0)) };
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
