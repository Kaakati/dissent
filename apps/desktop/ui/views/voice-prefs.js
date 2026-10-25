// Voice settings for the user bar popovers and the Settings Voice page. One copy lives in memory. A write is merged
// into it and sent after a short pause, so dragging a slider saves once, and saves go out in the order they were made.
import { api } from '../api.js';
import { toast } from '../ui.js';

let settings = null;
let timer = null;
let queue = Promise.resolve();

// Fetches the saved settings, replacing the copy in memory.
export async function loadVoice() {
  settings = await api.voiceSettings();
  return settings;
}

// The copy in memory, or null before the first load.
export function voiceSnapshot() {
  return settings;
}

// Merges `patch` into the settings and saves them. `now` skips the pause, for toggles and radio rows.
export function updateVoice(patch, { now = false } = {}) {
  settings = { ...settings, ...patch };
  clearTimeout(timer);
  timer = null;
  if (now) flush();
  else timer = setTimeout(flush, 150);
  return settings;
}

// Resolves once every save made so far has been sent, or reverted if it was refused.
export function settled() {
  return queue;
}

// Sends the current settings. A refused save, such as a key another app holds, reverts the copy to the saved one.
function flush() {
  clearTimeout(timer);
  timer = null;
  const snapshot = settings;
  queue = queue.then(() => api.setVoiceSettings(snapshot)).catch(async (e) => {
    toast(String(e?.message ?? e));
    await loadVoice().catch(() => {});
  });
  return queue;
}

export const PROFILES = [
  { value: 'voice_isolation', label: 'Voice Isolation', sub: 'Strong noise suppression' },
  { value: 'studio', label: 'Studio', sub: 'No processing' },
  { value: 'custom', label: 'Custom', sub: 'Choose each setting' },
];

// Windows calls the system choice "Windows Default", as its own menus do. Other systems say "System default".
export const systemLabel = () => (/Windows/.test(navigator.userAgent) ? 'Windows Default' : 'System default');

// The name a device row shows: the chosen device, or the default one when none is chosen.
export function shownDevice(list, chosen) {
  return chosen ?? list.find((d) => d.is_default)?.name ?? systemLabel();
}

// Saves a device choice. The other direction keeps its saved device. A call in progress moves to it.
export async function chooseDevice(direction, name) {
  const devices = await api.audioDevices();
  const input = direction === 'input' ? name : devices.input;
  const output = direction === 'output' ? name : devices.output;
  await api.setAudioDevices(input ?? null, output ?? null);
}
