// The microphone and output popovers on the user bar. Each is a context menu: a device picker with a submenu, the volume,
// and a shortcut to Settings › Voice. Opened from the chevron beside the mic or headphone button.
import { api } from '../api.js';
import { icon } from '../ui.js';
import { openContextMenu } from './contextmenu.js';
import { openSettingsPage } from './settings.js';
import { chooseDevice, loadVoice, PROFILES, shownDevice, systemLabel, updateVoice } from './voice-prefs.js';

// A headset gets headphones and a display gets a monitor. Anything else gets the fallback for its direction.
function deviceIcon(kind, fallback) {
  if (kind === 'headset') return icon('headphones');
  if (kind === 'display') return icon('monitor');
  return icon(fallback);
}

// The system default first, then every device, each a radio. `chosen` is the saved name, or null for the default.
function deviceItems(list, chosen, fallback, onChoose) {
  const defaultDevice = list.find((d) => d.is_default);
  return [
    {
      label: systemLabel(),
      sub: defaultDevice?.name,
      leading: deviceIcon(defaultDevice?.kind, fallback),
      radio: true,
      checked: chosen === null,
      onPick: () => onChoose(null),
    },
    ...list.map((d) => ({
      label: d.name,
      sub: d.subtitle ?? undefined,
      leading: deviceIcon(d.kind, fallback),
      radio: true,
      checked: chosen === d.name,
      onPick: () => onChoose(d.name),
    })),
  ];
}

// Moves the menu to sit above the anchor, which is the foot of the window.
function openAbove(anchor, items) {
  const r = anchor.getBoundingClientRect();
  openContextMenu(r.left, r.top - 6, items, { placeAbove: true });
}

export async function openOutputMenu(anchor) {
  const [devices, voice] = await Promise.all([api.audioDevices(), loadVoice()]);
  openAbove(anchor, [
    {
      label: 'Output Device',
      sub: shownDevice(devices.outputs, devices.output),
      leading: icon('headphones'),
      submenu: deviceItems(devices.outputs, devices.output, 'volume-2', (name) => chooseDevice('output', name)),
    },
    {
      slider: {
        label: 'Output Volume',
        min: 0,
        max: 200,
        value: voice.output_volume,
        format: (v) => `${v}%`,
        onInput: (v) => updateVoice({ output_volume: v }),
      },
    },
    'sep',
    { label: 'Voice Settings', icon: 'settings', onPick: () => openSettingsPage('voice') },
  ]);
}

export async function openInputMenu(anchor) {
  const [devices, voice] = await Promise.all([api.audioDevices(), loadVoice()]);
  const profile = PROFILES.find((p) => p.value === voice.profile) ?? PROFILES[0];
  openAbove(anchor, [
    {
      label: 'Input Device',
      sub: shownDevice(devices.inputs, devices.input),
      leading: icon('mic'),
      submenu: deviceItems(devices.inputs, devices.input, 'mic', (name) => chooseDevice('input', name)),
    },
    {
      label: 'Input Profile',
      sub: profile.label,
      leading: icon('mic'),
      submenu: PROFILES.map((p) => ({
        label: p.label,
        sub: p.sub,
        radio: true,
        checked: voice.profile === p.value,
        onPick: () => updateVoice({ profile: p.value }, { now: true }),
      })),
    },
    {
      slider: {
        label: 'Input Volume',
        min: 0,
        max: 200,
        value: voice.input_volume,
        format: (v) => `${v}%`,
        onInput: (v) => updateVoice({ input_volume: v }),
      },
    },
    'sep',
    { label: 'Voice Settings', icon: 'settings', onPick: () => openSettingsPage('voice') },
  ]);
}
