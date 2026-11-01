// How a screen share goes out, chosen in the Screen Share dialog and remembered on this device: the tallest picture (0 is the
// source's own size), the highest frame rate, and the bitrate. The engine holds the bitrate to the same range (SCREEN_MIN_KBPS and
// SCREEN_MAX_KBPS in dissent-video), and the link can still hold a share below any of them.
import { api } from './api';

const KEY = 'dissent.shareSettings';

export const SCREEN_BITRATE_MIN = 1500;
export const SCREEN_BITRATE_MAX = 6000;
export const SCREEN_BITRATE_STEP = 250;

export interface ShareSettings {
  height: 0 | 720 | 1080 | 1440;
  fps: 15 | 30 | 60;
  kbps: number;
}

// Better Readability keeps every pixel, and Smoother Video gives up some for frame rate.
export const PRESETS: { name: string; height: ShareSettings['height']; fps: ShareSettings['fps'] }[] = [
  { name: 'Better Readability', height: 0, fps: 30 },
  { name: 'Smoother Video', height: 1080, fps: 60 },
];

const DEFAULTS: ShareSettings = { height: 0, fps: 30, kbps: 3000 };

export function presetOf(s: ShareSettings): string {
  return PRESETS.find((p) => p.height === s.height && p.fps === s.fps)?.name ?? 'Custom';
}

export function savedShareSettings(): ShareSettings {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return {
      height: [0, 720, 1080, 1440].includes(saved.height) ? saved.height : DEFAULTS.height,
      fps: [15, 30, 60].includes(saved.fps) ? saved.fps : DEFAULTS.fps,
      kbps: saved.kbps >= SCREEN_BITRATE_MIN && saved.kbps <= SCREEN_BITRATE_MAX ? saved.kbps : DEFAULTS.kbps,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveShareSettings(s: ShareSettings) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // A window that cannot store the choice keeps it for this session only.
  }
}

export async function sendShareQuality(s: ShareSettings) {
  await api.setScreenBitrate(s.kbps);
  await api.setScreenQuality(s.height || null, s.fps);
}
