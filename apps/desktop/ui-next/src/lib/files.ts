// The server file tracker (Files area, Server Settings > Files). See `dissent_core::tracker`.
import { useEffect } from 'react';
import { create } from 'zustand';
import { onEngineEvent } from './api';
import type { FileDanger, Id, TransferItem } from './types';

export interface DownloadRules {
  min_ratio_milli: number | null;
  ratio_after_bytes: number;
  seed_hours: number | null;
}

export interface AddSpecView {
  min_rank: 'member' | 'moderator' | 'admin' | 'owner' | 'none';
  roles: Id[];
  members: Id[];
  min_level: number;
}

export interface CategoryView {
  id: Id;
  readable: boolean;
  name: string;
  icon: string;
  types: string[];
  default_tags: string[];
  rules: DownloadRules;
  add: AddSpecView;
  restricted: boolean;
  visible: ReaderSpecView | null;
  can_add: boolean;
  files: number;
  bytes: number;
  folder: string | null;
  auto_import: boolean;
}

export type LocalState = 'seeding' | 'paused' | 'downloading' | 'downloaded' | 'failed' | 'none';

export interface ListingView {
  id: Id;
  category: Id;
  name: string;
  ext: string;
  kind: string;
  mime: string;
  size: number;
  cipher_size: number;
  pieces: number;
  fingerprint: string;
  tags: string[];
  description: string;
  lister: Id;
  lister_name: string;
  added_ms: number;
  also_listed_by: string[];
  seeders: number;
  leechers: number;
  completed: number;
  mine: boolean;
  local: LocalState;
  local_path: string | null;
  danger?: FileDanger | null; // the file can run code: the page warns
  progress: number;
  can_edit: boolean;
}

export interface LevelRule {
  name: string;
  bytes: number;
  members: number;
}

export interface ReputationRules {
  levels: LevelRule[];
  distinct: boolean;
  decay_days: number;
  admins_reset: boolean;
}

export interface StandingView {
  level: number;
  level_name: string;
  next: { level: number; name: string; bytes_needed: number; members_needed: number; progress: number } | null;
  credited_bytes: number;
  served_members: number;
  uploaded: number;
  downloaded: number;
  ratio: number | null;
}

export interface FilesView {
  can_manage: boolean;
  categories: CategoryView[];
  listings: ListingView[];
  locked: number;
  standing: StandingView;
  rules: ReputationRules;
  all_types: string[];
}

export interface SwarmPeerView {
  root_id: Id;
  name: string;
  role: string;
  storage: boolean;
  online: boolean;
  has: number; // 0 to 1
  level: number;
}

export interface SwarmView {
  seeders: number;
  downloading: number;
  peers: SwarmPeerView[];
  pieces: number[]; // holders per 64 KiB piece, among `asked`
  asked: number;
}

export interface ReaderSpecView {
  min_rank: AddSpecView['min_rank'];
  roles: Id[];
  members: Id[];
}

export interface CategoryInput {
  name: string;
  icon: string;
  types: string[];
  default_tags: string[];
  rules: DownloadRules;
  add: AddSpecView;
  visible: ReaderSpecView | null; // null: every member; fixed once the category is made
}

export type CandidateState = 'ready' | 'already_listed' | 'type_not_allowed' | 'too_large' | 'unreadable';

export interface CandidateView {
  path: string;
  name: string;
  ext: string;
  kind: string;
  size: number;
  state: CandidateState;
  fingerprint: string;
  listed_by: string | null;
  listing: Id | null;
}

// A chosen path, as far as it can be told without reading the file.
export interface PickedView {
  path: string;
  name: string;
  ext: string;
  kind: string;
  size: number;
  is_dir: boolean;
}

// A dropped file, and where it goes or why it does not (`DropItem` in dissent-core).
export interface DropItem {
  path: string;
  name: string;
  ext: string;
  kind: string;
  size: number;
  category: Id | null;
  category_name: string | null;
  skip: string | null;
}

export interface DropPlan {
  items: DropItem[];
  folders: string[];
  created: string[];
  no_categories: boolean;
}

export interface AddItem {
  path: string;
  tags: string[];
  description: string;
}

const invoke = <T>(cmd: string, args: Record<string, unknown>): Promise<T> => {
  const tauri = window.__TAURI__;
  if (!tauri) return Promise.reject(new Error('No backend: the app has not loaded the Tauri globals or the mock.'));
  return tauri.core.invoke<T>(cmd, args);
};

export const filesApi = {
  view: (communityId: Id) => invoke<FilesView>('files_view', { communityId }),
  createCategory: (communityId: Id, category: CategoryInput) => invoke<Id>('create_category', { communityId, category }),
  updateCategory: (communityId: Id, categoryId: Id, category: CategoryInput) => invoke<void>('update_category', { communityId, categoryId, category }),
  deleteCategory: (communityId: Id, categoryId: Id) => invoke<void>('delete_category', { communityId, categoryId }),
  pickFolder: (communityId: Id, categoryId: Id) => invoke<string | null>('pick_category_folder', { communityId, categoryId }),
  unlinkFolder: (communityId: Id, categoryId: Id) => invoke<void>('unlink_category_folder', { communityId, categoryId }),
  setAutoImport: (communityId: Id, categoryId: Id, on: boolean) => invoke<void>('set_category_auto_import', { communityId, categoryId, on }),
  pickFiles: () => invoke<PickedView[]>('pick_listing_files', {}),
  scanFolder: (communityId: Id, categoryId: Id) => invoke<PickedView[]>('scan_category_folder', { communityId, categoryId }),
  startCheck: (communityId: Id, paths: string[]) => invoke<number>('start_file_check', { communityId, paths }),
  cancelCheck: (job: number) => invoke<void>('cancel_file_check', { job }),
  planDrop: (communityId: Id, paths: string[], filter: Id | null) => invoke<DropPlan>('plan_file_drop', { communityId, paths, filter }),
  list: (communityId: Id, categoryId: Id, items: AddItem[], limit: string, inPlace = false) => invoke<Id[]>('list_files', { communityId, categoryId, items, limit, inPlace }),
  updateListing: (communityId: Id, listingId: Id, tags: string[], description: string) =>
    invoke<void>('update_listing', { communityId, listingId, tags, description }),
  removeListing: (communityId: Id, listingId: Id) => invoke<void>('remove_listing', { communityId, listingId }),
  download: (communityId: Id, listingId: Id) => invoke<TransferItem>('download_listing', { communityId, listingId }),
  swarm: (communityId: Id, listingId: Id) => invoke<SwarmView>('listing_swarm', { communityId, listingId }),
  setRules: (communityId: Id, rules: ReputationRules) => invoke<void>('set_reputation_rules', { communityId, rules }),
  resetReputation: (communityId: Id, userId: Id) => invoke<void>('reset_reputation', { communityId, userId }),
};

interface FileCheckEvents {
  progress?: (path: string, done: number, total: number) => void;
  result: (candidate: CandidateView) => void;
  done?: () => void;
  failed?: (err: unknown) => void;
}

// Fingerprints `paths` in the background (`start_file_check`) and reports each. Returns what stops it, which also stops
// the work in the engine.
export function checkFiles(communityId: Id, paths: string[], on: FileCheckEvents): () => void {
  let job: number | null = null;
  let stopped = false;
  // Events can arrive before the job's number does.
  const early: Parameters<Parameters<typeof onEngineEvent>[0]>[0][] = [];
  const handle = (ev: Parameters<Parameters<typeof onEngineEvent>[0]>[0]) => {
    if (stopped) return;
    if (ev.kind !== 'file_check' && ev.kind !== 'file_check_done') return;
    if (job === null) {
      early.push(ev);
      return;
    }
    if (ev.job !== job) return;
    if (ev.kind === 'file_check_done') {
      on.done?.();
      return;
    }
    if (ev.result) on.result(ev.result);
    else on.progress?.(ev.path, ev.done, ev.total);
  };
  const listening = onEngineEvent(handle);
  void listening
    .then(() => filesApi.startCheck(communityId, paths))
    .then((id) => {
      job = id;
      if (stopped) void filesApi.cancelCheck(id).catch(() => {});
      else early.splice(0).forEach(handle);
    })
    .catch((err) => {
      if (!stopped) on.failed?.(err);
    });
  return () => {
    stopped = true;
    if (job !== null) void filesApi.cancelCheck(job).catch(() => {});
    void listening.then((unlisten) => unlisten());
  };
}

// The open server's files view. Reloaded on every change of that server and of the transfers.
export const useFiles = create<{ communityId: Id | null; view: FilesView | null; error: string | null }>(() => ({
  communityId: null,
  view: null,
  error: null,
}));

let loading = 0;
export async function reloadFiles(communityId: Id): Promise<void> {
  const ticket = ++loading;
  try {
    const view = await filesApi.view(communityId);
    if (ticket === loading) useFiles.setState({ communityId, view, error: null });
  } catch (err) {
    if (ticket === loading) useFiles.setState({ communityId, error: String(err) });
  }
}

// Loads the files view of `communityId`, and again whenever `community` or `transfers` changes (pass the store's
// objects, which change identity on each update).
export function useFilesOf(communityId: Id | null, community: unknown, transfers?: unknown): FilesView | null {
  useEffect(() => {
    if (communityId) void reloadFiles(communityId);
  }, [communityId, community, transfers]);
  return useFiles((s) => (s.communityId === communityId ? s.view : null));
}

// The type tile's hue, as in the artboards.
const HUE: Record<string, number> = { Installer: 25, Archive: 60, Document: 250, Image: 150, Audio: 300, Video: 330, Other: 220 };
export const tileStyle = (kind: string) => ({ background: `oklch(45% 0.14 ${HUE[kind] ?? 220})` });

export const formatLevel = (s: StandingView): string => `Level ${s.level}${s.level_name ? ` · ${s.level_name}` : ''}`;

export interface AltSchedule {
  days: number; // bit 0 Monday … bit 6 Sunday
  start_min: number;
  end_min: number;
  upload_bps: number | null;
  download_bps: number | null;
}

export interface SeedingSettings {
  upload_bps: number | null;
  download_bps: number | null;
  alt: AltSchedule | null;
  alt_on: boolean;
  max_active: number;
  unmetered_only: boolean;
  pause_on_battery: boolean;
}

export type SeedState = 'seeding' | 'paused' | 'scheduled' | 'queued' | 'stopped';

export interface SeedRowView {
  blob: string;
  listing: Id;
  name: string;
  ext: string;
  kind: string;
  category: Id;
  folder: string | null;
  path: string | null;
  state: SeedState;
  note: string;
  size: number;
  uploaded: number;
  ratio: number;
  limit: string; // 'forever' | 'ratio:<thousandths>' | 'hours:<n>' | 'until:<ms>'
  since_ms: number;
  downloaded: boolean;
}

export interface SeedingView {
  rows: SeedRowView[];
  seeding: number;
  uploaded: number;
  settings: SeedingSettings;
  conditions: { metered: boolean; on_battery: boolean; metered_known: boolean };
  caps_now: [number | null, number | null];
  held_back: boolean;
}

export const seedingApi = {
  view: (communityId: Id) => invoke<SeedingView>('seeding_view', { communityId }),
  setSettings: (settings: SeedingSettings) => invoke<void>('set_seeding_settings', { settings }),
  setLimit: (communityId: Id, blob: string, limit: string) => invoke<void>('set_seed_limit', { communityId, blob, limit }),
  pause: (communityId: Id, blob: string, paused: boolean) => invoke<void>('pause_seed', { communityId, blob, paused }),
  stop: (communityId: Id, blob: string) => invoke<void>('stop_seed', { communityId, blob }),
  pauseAll: (communityId: Id, paused: boolean) => invoke<void>('pause_all_seeds', { communityId, paused }),
  setUtcOffset: (minutes: number) => invoke<void>('set_utc_offset', { minutes }),
};

// A seed limit as the row shows it.
export function limitLabel(limit: string): string {
  const [kind, value] = limit.split(':');
  if (kind === 'ratio') return `Until ratio ${(Number(value) / 1000).toFixed(1)}`;
  if (kind === 'hours') return `For ${value} h`;
  if (kind === 'until') return `Until ${new Date(Number(value)).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`;
  return 'Forever';
}
