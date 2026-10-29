import { api } from './api';

// Bumped by every copy, so a pending clear (copySecret) knows whether something else was copied since.
let copies = 0;

// Copies text to the clipboard. Falls back to a selected textarea where the async API is refused (the old UI's ui.js copyText).
export async function copyText(text: string): Promise<boolean> {
  copies++;
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const field = document.createElement('textarea');
    field.value = text;
    document.body.append(field);
    field.select();
    const ok = document.execCommand('copy');
    field.remove();
    return ok;
  }
}

export const SECRET_CLEAR_MS = 60_000;

// Copies a secret (the recovery phrase) and clears the clipboard after 60 s if it still holds it. The desktop app copies
// it natively (copy_secret), which keeps it out of Windows clipboard history and cloud sync and marks it concealed for
// clipboard managers on macOS and Linux; the clear is native too (clear_secret). Without those commands it falls back to
// the web clipboard, which cannot opt out of the history; where that clipboard cannot be read back, it is cleared unless
// something else was copied from this app since.
export async function copySecret(text: string): Promise<boolean> {
  let native = true;
  try {
    await api.copySecret(text);
    copies++;
  } catch {
    native = false;
    if (!(await copyText(text))) return false;
  }
  const ms = (window.__DISSENT_MOCK__ && window.__mockSecretClearMs) || SECRET_CLEAR_MS;
  const mine = copies;
  window.setTimeout(() => void (native ? clearNative(text) : clearWeb(text, mine)), ms);
  return true;
}

async function clearNative(text: string): Promise<void> {
  try {
    await api.clearSecret(text);
  } catch {
    // The clipboard was busy or unavailable: the phrase stays until something else is copied.
  }
}

async function clearWeb(text: string, mine: number): Promise<void> {
  let holds: boolean;
  try {
    holds = (await navigator.clipboard.readText()) === text;
  } catch {
    holds = copies === mine;
  }
  if (!holds) return;
  try {
    await navigator.clipboard.writeText('');
  } catch {
    // Refused (the window is not focused): the phrase stays until something else is copied.
  }
}
