// File size limits: the largest file this device shares and downloads (Settings › Downloads), and the largest a server
// lets its members share (Server Settings › Files). See dissent_core::limits. Units are binary, as the engine counts them.
import { useEffect } from 'react';
import { create } from 'zustand';
import { confirm } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { Id } from '@/lib/types';

export const MB = 1024 ** 2;
export const GB = 1024 ** 3;
export const TB = 1024 ** 4;
// 1 MB to the file format's ceiling of 1 TB, and 10 GB until changed.
export const MIN_LIMIT = MB;
export const MAX_LIMIT = TB;
export const DEFAULT_LIMIT = 10 * GB;
// The most Dissent 0.1.35 and earlier accept: a larger file does not reach members on those versions.
export const RELEASED_LIMIT = 10 * GB;
export const OLDER_VERSIONS_NOTE = 'Members on Dissent 0.1.35 or earlier can’t download files over 10 GB.';

export type SizeUnit = 'MB' | 'GB' | 'TB';
export const UNITS: Record<SizeUnit, number> = { MB, GB, TB };

export interface SizeLimits {
  max_upload: number;
  max_download: number;
}

// A limit as people read it: "10 GB", "1.5 GB", "512 MB".
export function limitLabel(bytes: number): string {
  const unit: SizeUnit = bytes >= TB && bytes % TB === 0 ? 'TB' : bytes >= GB ? (bytes >= TB ? 'TB' : 'GB') : 'MB';
  const value = bytes / UNITS[unit];
  return `${Number.isInteger(value) ? value : Number(value.toFixed(1))} ${unit}`;
}

// The unit a stored limit is shown in, and its number in that unit.
export function splitLimit(bytes: number): { value: string; unit: SizeUnit } {
  const unit: SizeUnit = bytes >= TB ? 'TB' : bytes >= GB ? 'GB' : 'MB';
  const value = bytes / UNITS[unit];
  return { value: String(Number.isInteger(value) ? value : Number(value.toFixed(3))), unit };
}

// The bytes a typed number and unit mean, or why they are not a limit.
export function parseLimit(value: string, unit: SizeUnit): { bytes: number } | { error: string } {
  const n = Number(value.trim());
  if (!value.trim() || !Number.isFinite(n) || n <= 0) return { error: 'Enter a size.' };
  const bytes = Math.round(n * UNITS[unit]);
  if (bytes < MIN_LIMIT) return { error: 'The smallest limit is 1 MB.' };
  if (bytes > MAX_LIMIT) return { error: 'The largest limit is 1 TB.' };
  return { bytes };
}

const invoke = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> => {
  const tauri = window.__TAURI__;
  if (!tauri) return Promise.reject(new Error('No backend: the app has not loaded the Tauri globals or the mock.'));
  return tauri.core.invoke<T>(cmd, args);
};

export const limitsApi = {
  get: () => invoke<SizeLimits>('size_limits'),
  set: (limits: SizeLimits) => invoke<SizeLimits>('set_size_limits', { limits }),
  setServer: (communityId: Id, bytes: number) => invoke<void>('set_server_file_size_limit', { communityId, bytes }),
};

export const useSizeLimits = create<{ limits: SizeLimits | null; load: () => Promise<void>; save: (limits: SizeLimits) => Promise<boolean> }>()((set) => ({
  limits: null,
  load: async () => {
    try {
      set({ limits: await limitsApi.get() });
    } catch {
      set({ limits: { max_upload: DEFAULT_LIMIT, max_download: DEFAULT_LIMIT } });
    }
  },
  save: async (limits) => {
    try {
      set({ limits: await limitsApi.set(limits) });
      return true;
    } catch (err) {
      toast(err);
      return false;
    }
  },
}));

// The largest file this device may send where it is now: its own upload limit, and in a server, that server's too.
export function useUploadLimit(): number {
  const known = useSizeLimits((s) => s.limits !== null);
  const load = useSizeLimits((s) => s.load);
  // Read once, the first time anything asks; the settings page keeps it current after that.
  useEffect(() => {
    if (!known) void load();
  }, [known, load]);
  const mine = useSizeLimits((s) => s.limits?.max_upload ?? DEFAULT_LIMIT);
  const server = useShell((s) => (s.route.kind === 'server' ? s.community?.max_file_size : undefined));
  return typeof server === 'number' ? Math.min(mine, server) : mine;
}

export const isOver = (file: { size: number | null | undefined }, limit: number): boolean =>
  file.size !== null && file.size !== undefined && Number(file.size) > limit;

// Runs a download, and when it is over this device's download limit, asks before going ahead anyway.
export async function downloadWithin<T>(start: (force: boolean) => Promise<T>): Promise<T | null> {
  try {
    return await start(false);
  } catch (err) {
    const text = String(err);
    if (!text.includes('over_download_limit')) throw err;
    const note = text.replace(/^.*over_download_limit:\s*/, '');
    const ok = await confirm({
      title: 'Download anyway?',
      message: `${note}. You can change it in Settings › Downloads.`,
      okLabel: 'Download anyway',
    });
    return ok ? start(true) : null;
  }
}
