// Push to talk keys as accelerators, such as Control+KeyV (the form voice_settings stores). Ported from
// ui/views/settings-voice.js (keyLabel, the binder's key handling).
const MODIFIERS = new Set([
  'ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight', 'OSLeft', 'OSRight', 'CapsLock',
]);

// An accelerator as a short label: Control+KeyV reads "Ctrl + V".
export function keyLabel(accel: string | null | undefined): string {
  if (!accel) return 'Set a key';
  return accel
    .split('+')
    .map((part) => part.replace('Control', 'Ctrl').replace('Super', 'Win').replace(/^Key/, '').replace(/^Digit/, '').replace(/^Numpad/, 'Num '))
    .join(' + ');
}

// The accelerator of a key press, or null for a modifier on its own (it waits for the key it is held with).
export function acceleratorOf(e: KeyboardEvent): string | null {
  if (MODIFIERS.has(e.code) || !e.code) return null;
  const modifiers = [e.ctrlKey && 'Control', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Super'].filter(Boolean);
  return [...modifiers, e.code].join('+');
}
