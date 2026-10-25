// Voice settings and audio devices, for the user bar's popovers and Settings › Voice. One copy lives in the store. A write
// is merged into it and sent after a short pause, so dragging a slider saves once, and saves go out in the order they
// were made. A refused save reverts the copy to the saved one. Ported from ui/views/voice-prefs.js.
import { create } from 'zustand';
import { api } from './api';
import { toast } from './toasts';
import type { AudioDevice, VoiceProfile, VoiceSettings } from './types';

interface VoiceState {
  settings: VoiceSettings | null; // null until the first load
}

export const useVoice = create<VoiceState>()(() => ({ settings: null }));

const SAVE_PAUSE_MS = 150;
let timer: ReturnType<typeof setTimeout> | null = null;
let queue: Promise<void> = Promise.resolve();

// Fetches the saved settings, replacing the copy in the store.
export async function loadVoice(): Promise<VoiceSettings> {
  const settings = await api.voiceSettings();
  useVoice.setState({ settings });
  return settings;
}

// Merges `patch` into the settings and saves them. `now` skips the pause, for toggles and radio rows.
export function updateVoice(patch: Partial<VoiceSettings>, { now = false }: { now?: boolean } = {}): void {
  const current = useVoice.getState().settings;
  if (!current) return;
  useVoice.setState({ settings: { ...current, ...patch } });
  if (timer !== null) clearTimeout(timer);
  timer = null;
  if (now) flush();
  else timer = setTimeout(flush, SAVE_PAUSE_MS);
}

// Resolves once every save made so far has been sent, or reverted if it was refused.
export function settled(): Promise<void> {
  return queue;
}

function flush(): void {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  const snapshot = useVoice.getState().settings;
  if (!snapshot) return;
  queue = queue
    .then(() => api.setVoiceSettings(snapshot))
    .catch(async (err) => {
      toast(err);
      await loadVoice().catch(() => {});
    });
}

export const PROFILES: { value: VoiceProfile; label: string; sub: string }[] = [
  { value: 'voice_isolation', label: 'Voice Isolation', sub: 'Strong noise suppression' },
  { value: 'studio', label: 'Studio', sub: 'No processing' },
  { value: 'custom', label: 'Custom', sub: 'Choose each setting' },
];

// Windows calls the system choice "Windows Default", as its own menus do. Other systems say "System default".
export const systemLabel = (): string => (/Windows/.test(navigator.userAgent) ? 'Windows Default' : 'System default');

// The name a device row shows: the chosen device, or the default one when none is chosen.
export function shownDevice(list: AudioDevice[], chosen: string | null): string {
  return chosen ?? list.find((d) => d.is_default)?.name ?? systemLabel();
}

// Saves a device choice. The other direction keeps its saved device. A call in progress moves to it.
export async function chooseDevice(direction: 'input' | 'output', name: string | null): Promise<void> {
  const devices = await api.audioDevices();
  const input = direction === 'input' ? name : devices.input;
  const output = direction === 'output' ? name : devices.output;
  await api.setAudioDevices(input, output);
}
