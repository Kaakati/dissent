// Transfer actions for the Transfers page and the file details. Each calls the backend, then reloads what the page and
// the sidebar badge read. A failure becomes a toast and never throws. Ported from the transfers section of the old UI's actions.js.
import { create } from 'zustand';
import { api } from './api';
import { has } from './features';
import { useShell } from './store';
import { toast } from './toasts';
import type { Id, TransferSettings } from './types';

// The download folder and the auto-accept limit. Null until the backend answers, or when it does not report them.
interface TransferSettingsState {
  settings: TransferSettings | null;
  load: () => Promise<void>;
}

export const useTransferSettings = create<TransferSettingsState>()((set) => ({
  settings: null,
  load: async () => {
    try {
      set({ settings: await api.transferSettings() });
    } catch {
      set({ settings: null }); // the Storage panel then shows no folder and no Change button
    }
  },
}));

async function reload(): Promise<void> {
  if (!has('transfers')) return;
  await useShell.getState().loadTransfers();
  await useTransferSettings.getState().load();
}

// Runs one backend call and reloads on success. A failure is a toast.
async function run(fn: () => Promise<unknown>): Promise<void> {
  try {
    await fn();
    await reload();
  } catch (err) {
    toast(err);
  }
}

export const pauseTransfer = (id: Id) => run(() => api.pauseTransfer(id));
export const resumeTransfer = (id: Id) => run(() => api.resumeTransfer(id));
export const cancelTransfer = (id: Id) => run(() => api.cancelTransfer(id));
export const retryTransfer = (id: Id) => run(() => api.retryTransfer(id));
export const removeTransfer = (id: Id) => run(() => api.removeTransfer(id));

// Opens the folder with a finished file selected. The backend accepts only files Dissent saved.
export async function revealTransfer(path: string): Promise<void> {
  try {
    await api.revealAttachment(path);
  } catch (err) {
    toast(err);
  }
}

// Asks for a new download folder and saves it. Cancelling the picker keeps the current folder.
export async function changeDownloadDir(): Promise<void> {
  try {
    useTransferSettings.setState({ settings: await api.pickDownloadDir() });
  } catch (err) {
    toast(err);
  }
}

// Goes back to the default download folder, which follows the OS Downloads folder.
export async function resetDownloadDir(): Promise<void> {
  try {
    useTransferSettings.setState({ settings: await api.resetDownloadDir() });
  } catch (err) {
    toast(err);
  }
}

// Files up to this size download without asking. Null means always ask.
export async function setAutoAccept(bytes: number | null): Promise<void> {
  const settings = useTransferSettings.getState().settings;
  if (!settings) return;
  await run(() => api.setTransferSettings({ ...settings, auto_accept_bytes: bytes }));
}
