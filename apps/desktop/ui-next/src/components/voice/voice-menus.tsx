// The microphone and output popovers on the user bar (the old UI's views/voice-menus.js). Each is a context menu opened from the
// chevron beside the mic or headphone button: a device picker with a radio row per device, the input profile, the volume,
// and a shortcut to Settings › Voice. A headset gets headphones and a display gets a monitor; anything else gets the
// microphone or speaker icon of its direction.
import { Headphones, Mic, Monitor, Settings, Volume2 } from 'lucide-react';
import { openContextMenu, type MenuEntry, type MenuItem } from '@/lib/menu';
import { api } from '@/lib/api';
import { toast } from '@/lib/toasts';
import { useShell } from '@/lib/store';
import { loadVoice, PROFILES, shownDevice, systemLabel, chooseDevice, updateVoice } from '@/lib/voice';
import type { AudioDevice, VoiceSettings } from '@/lib/types';

const ICON = 'size-4 shrink-0';

function deviceIcon(kind: AudioDevice['kind'] | undefined, fallback: 'mic' | 'speaker') {
  if (kind === 'headset') return <Headphones className={ICON} aria-hidden="true" />;
  if (kind === 'display') return <Monitor className={ICON} aria-hidden="true" />;
  return fallback === 'mic' ? <Mic className={ICON} aria-hidden="true" /> : <Volume2 className={ICON} aria-hidden="true" />;
}

// The system default first, then every device, each a radio. `chosen` is the saved name, or null for the default.
function deviceItems(
  list: AudioDevice[],
  chosen: string | null,
  fallback: 'mic' | 'speaker',
  direction: 'input' | 'output',
): MenuEntry[] {
  const defaultDevice = list.find((d) => d.is_default);
  const choose = (name: string | null) => {
    chooseDevice(direction, name).catch(toast);
  };
  return [
    {
      label: systemLabel(),
      sub: defaultDevice?.name,
      leading: deviceIcon(defaultDevice?.kind, fallback),
      radio: true,
      checked: chosen === null,
      onPick: () => choose(null),
    },
    ...list.map<MenuItem>((d) => ({
      label: d.name,
      sub: d.subtitle ?? undefined,
      leading: deviceIcon(d.kind, fallback),
      radio: true,
      checked: chosen === d.name,
      onPick: () => choose(d.name),
    })),
  ];
}

// Opens the menu above the anchor, which sits at the foot of the window.
function openAbove(anchor: HTMLElement, items: MenuEntry[]): void {
  const r = anchor.getBoundingClientRect();
  openContextMenu(r.left, r.top - 6, items, { placeAbove: true });
}

const openVoiceSettings = (): void => useShell.getState().openSettings('voice');

export async function openOutputMenu(anchor: HTMLElement): Promise<void> {
  try {
    const [devices, voice] = await Promise.all([api.audioDevices(), loadVoice()]);
    openAbove(anchor, [
      {
        label: 'Output Device',
        sub: shownDevice(devices.outputs, devices.output),
        leading: <Headphones className={ICON} aria-hidden="true" />,
        submenu: deviceItems(devices.outputs, devices.output, 'speaker', 'output'),
      },
      volumeSlider('Output Volume', voice, 'output_volume'),
      'sep',
      { label: 'Voice Settings', icon: Settings, onPick: openVoiceSettings },
    ]);
  } catch (err) {
    toast(err);
  }
}

export async function openInputMenu(anchor: HTMLElement): Promise<void> {
  try {
    const [devices, voice] = await Promise.all([api.audioDevices(), loadVoice()]);
    const profile = PROFILES.find((p) => p.value === voice.profile) ?? PROFILES[0];
    openAbove(anchor, [
      {
        label: 'Input Device',
        sub: shownDevice(devices.inputs, devices.input),
        leading: <Mic className={ICON} aria-hidden="true" />,
        submenu: deviceItems(devices.inputs, devices.input, 'mic', 'input'),
      },
      {
        label: 'Input Profile',
        sub: profile.label,
        leading: <Mic className={ICON} aria-hidden="true" />,
        submenu: PROFILES.map<MenuItem>((p) => ({
          label: p.label,
          sub: p.sub,
          radio: true,
          checked: voice.profile === p.value,
          onPick: () => updateVoice({ profile: p.value }, { now: true }),
        })),
      },
      volumeSlider('Input Volume', voice, 'input_volume'),
      'sep',
      { label: 'Voice Settings', icon: Settings, onPick: openVoiceSettings },
    ]);
  } catch (err) {
    toast(err);
  }
}

// A volume row, 0 to 200 percent. Each step saves through the same debounced path as the settings page.
function volumeSlider(label: string, voice: VoiceSettings, key: 'input_volume' | 'output_volume'): MenuEntry {
  return {
    slider: {
      label,
      min: 0,
      max: 200,
      value: voice[key],
      format: (v: number) => `${v}%`,
      onInput: (v: number) => updateVoice({ [key]: v }),
    },
  };
}
