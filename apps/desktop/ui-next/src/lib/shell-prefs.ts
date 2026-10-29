// The desktop shell's settings (keep running, start at login, mute notifications) as a store that the settings pages and
// the notification code read. The desktop app keeps them and tells the window about changes made from the tray. The
// browser preview has no desktop app, so it keeps the defaults.
import { create } from 'zustand';
import { api } from './api';
import { toast } from './toasts';
import type { ShellSettings } from './types';

interface ShellPrefs {
  keepRunning: boolean;
  startAtLogin: boolean;
  startAtLoginAvailable: boolean;
  startAtLoginNote: string | null;
  notificationsMuted: boolean;
  convertEmoticons: boolean; // the composer turns :D and the like into emoji
  stripPhotoMetadata: boolean; // photos and videos go out without their location and camera details
  apply: (s: ShellSettings) => void;
  load: () => Promise<void>;
}

export const useShellPrefs = create<ShellPrefs>()((set) => ({
  keepRunning: false,
  startAtLogin: false,
  startAtLoginAvailable: false,
  startAtLoginNote: null,
  notificationsMuted: false,
  convertEmoticons: true,
  stripPhotoMetadata: true,
  apply: (s) =>
    set({
      keepRunning: s.keep_running,
      startAtLogin: s.start_at_login,
      startAtLoginAvailable: s.start_at_login_available,
      startAtLoginNote: s.start_at_login_note,
      notificationsMuted: s.notifications_muted,
      convertEmoticons: s.emoticons_as_text !== true,
      stripPhotoMetadata: s.keep_photo_metadata !== true,
    }),
  load: async () => {
    try {
      useShellPrefs.getState().apply(await api.shellSettings());
    } catch {
      // No desktop app behind the window: keep the defaults.
    }
  },
}));

type Change = Partial<Pick<ShellSettings, 'keep_running' | 'start_at_login' | 'notifications_muted' | 'emoticons_as_text' | 'keep_photo_metadata'>>;

// Saves one change with the rest of the settings. The backend answers with what it saved, which is what the page shows.
export async function setShellPrefs(change: Change): Promise<void> {
  const current = useShellPrefs.getState();
  const next = {
    keep_running: current.keepRunning,
    start_at_login: current.startAtLogin,
    notifications_muted: current.notificationsMuted,
    emoticons_as_text: !current.convertEmoticons,
    keep_photo_metadata: !current.stripPhotoMetadata,
    ...change,
  };
  try {
    useShellPrefs.getState().apply(await api.setShellSettings(next));
  } catch (err) {
    toast(err);
  }
}
