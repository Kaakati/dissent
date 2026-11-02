// Files added without a dialog: a drop on the Files page lists each file in the category its type fits, in the background.
// Each batch shows its files in a tray above the toasts, and its toast says what went where, with Undo, since a drop
// publishes to the whole server. See docs/design/14-file-tracker.md, "Adding files".
import { useState } from 'react';
import { Upload, X } from 'lucide-react';
import { create } from 'zustand';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { openDialog } from '@/lib/dialog';
import { checkFiles, filesApi, reloadFiles, seedingApi, type CandidateView, type ListingView } from '@/lib/files';
import { useShell } from '@/lib/store';
import { toast, type ToastAction } from '@/lib/toasts';
import { openAddFiles } from './add-files-dialog';
import { openAddFolder } from './add-folder-dialog';
import { useFilesUi } from './files-page';

type RowState = 'waiting' | 'checking' | 'listing' | 'added' | 'seeded' | 'skipped' | 'failed';

export interface AddRow {
  path: string;
  name: string;
  size: number;
  category: string | null;
  categoryName: string | null;
  state: RowState;
  progress: number;
  skip: string | null; // why it was not listed: see SKIP_TEXT
  listing: string | null;
}

interface AddBatch {
  id: number;
  communityId: string;
  rows: AddRow[];
  cancel: () => void;
}

export const useFileAdds = create<{ batches: AddBatch[]; dropOver: boolean }>(() => ({ batches: [], dropOver: false }));

const SKIP_TEXT: Record<string, string> = {
  no_category: 'fits no category',
  too_large: 'over 10 GB',
  unreadable: 'can’t be read',
  link: 'a link, not followed',
  credentials: 'looks like a key or password file',
  duplicate: 'already seeded by you',
  type_not_allowed: 'type not allowed',
  cancelled: 'cancelled',
};

const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// "3 files added to Images, 1 to Documents. Seeding 1 already listed. Not added: 2 fit no category."
export function summarize(rows: AddRow[]): string {
  const parts: string[] = [];
  const added = new Map<string, number>();
  for (const r of rows) if (r.state === 'added') added.set(r.categoryName ?? '', (added.get(r.categoryName ?? '') ?? 0) + 1);
  const places = [...added.entries()].sort((a, b) => b[1] - a[1]);
  if (places.length) parts.push(`${places.map(([name, n], i) => (i === 0 ? `${count(n, 'file')} added to ${name}` : `${n} to ${name}`)).join(', ')}.`);
  const seeded = rows.filter((r) => r.state === 'seeded').length;
  if (seeded) parts.push(`Seeding ${count(seeded, 'file')} already listed.`);
  const skipped = new Map<string, number>();
  for (const r of rows) if (r.state === 'skipped' && r.skip) skipped.set(r.skip, (skipped.get(r.skip) ?? 0) + 1);
  if (skipped.size) parts.push(`Not added: ${[...skipped.entries()].map(([why, n]) => `${n} ${SKIP_TEXT[why] ?? why}`).join(', ')}.`);
  const failed = rows.filter((r) => r.state === 'failed').length;
  if (failed) parts.push(`${count(failed, 'file')} could not be added.`);
  return parts.join(' ') || 'Nothing was added.';
}

let batchSeq = 0;

const patch = (id: number, path: string, change: Partial<AddRow>) =>
  useFileAdds.setState((s) => ({
    batches: s.batches.map((b) => (b.id === id ? { ...b, rows: b.rows.map((r) => (r.path === path ? { ...r, ...change } : r)) } : b)),
  }));
const batchOf = (id: number) => useFileAdds.getState().batches.find((b) => b.id === id);

// Takes back what a batch did: its new listings go, and a listing it only joined stops being seeded here.
async function undoBatch(communityId: string, rows: AddRow[]): Promise<void> {
  try {
    for (const r of rows) if (r.state === 'added' && r.listing) await filesApi.removeListing(communityId, r.listing);
    const seeded = rows.filter((r) => r.state === 'seeded' && r.listing);
    if (seeded.length) {
      const view = await seedingApi.view(communityId);
      for (const r of seeded) {
        const row = view.rows.find((x) => x.listing === r.listing);
        if (row) await seedingApi.stop(communityId, row.blob);
      }
    }
    await reloadFiles(communityId);
    toast(`Took back ${count(rows.filter((r) => r.state === 'added' || r.state === 'seeded').length, 'file')}`, 'info');
  } catch (err) {
    toast(err);
  }
}

// Checks and lists `rows` in the background, one listing at a time. A row that already has a `skip` is reported, not read.
// `checked` holds what a dialog already checked, so nothing is read twice.
export function startAdds(communityId: string, rows: AddRow[], options: { tagsFor: (row: AddRow) => string[]; description?: string; inPlace?: boolean; checked?: CandidateView[] }): void {
  const id = ++batchSeq;
  const todo = rows.filter((r) => !r.skip);
  const queue: CandidateView[] = [];
  let pumping = false;
  let checkDone = false;
  let cancelled = false;
  let stopCheck = () => {};

  const finish = () => {
    const batch = batchOf(id);
    if (!batch) return;
    useFileAdds.setState((s) => ({ batches: s.batches.filter((b) => b.id !== id) }));
    void reloadFiles(communityId);
    const done = batch.rows.filter((r) => r.state === 'added' || r.state === 'seeded');
    const left = batch.rows.filter((r) => r.state === 'skipped' && r.skip !== 'cancelled' && r.skip !== 'duplicate');
    const actions: ToastAction[] = [];
    if (done.length) actions.push({ label: 'Undo', run: () => void undoBatch(communityId, batch.rows) });
    if (done.length === 1 && done[0].state === 'added' && done[0].listing) {
      const listing = done[0].listing;
      actions.push({ label: 'Add tags', run: () => void editListingById(communityId, listing) });
    }
    if (left.length) actions.push({ label: 'Add files', run: () => void addFilesManually(communityId) });
    toast(summarize(batch.rows), done.length || !left.length ? 'info' : 'error', { actions, ms: 15000 });
  };
  const maybeFinish = () => {
    if (checkDone && !pumping && queue.length === 0) finish();
  };
  const pump = async () => {
    if (pumping) return;
    pumping = true;
    while (queue.length && !cancelled) {
      const c = queue.shift() as CandidateView;
      const row = batchOf(id)?.rows.find((r) => r.path === c.path);
      if (!row) continue;
      if (c.state !== 'ready' && c.state !== 'already_listed') {
        patch(id, c.path, { state: 'skipped', skip: c.state });
        continue;
      }
      try {
        // A file this device already seeds is left alone, so its seed keeps its counters.
        const existing = c.listing ? (await filesApi.view(communityId)).listings.find((l) => l.id === c.listing) : undefined;
        if (existing && (existing.local === 'seeding' || existing.local === 'paused')) {
          patch(id, c.path, { state: 'skipped', skip: 'duplicate' });
          continue;
        }
        patch(id, c.path, { state: 'listing' });
        const item = { path: c.path, tags: options.tagsFor(row), description: options.description ?? '' };
        const [listing] = await filesApi.list(communityId, row.category as string, [item], 'forever', options.inPlace);
        const again = batchOf(id)?.rows.some((r) => r.path !== c.path && r.listing === listing);
        if (again) patch(id, c.path, { state: 'skipped', skip: 'duplicate' });
        else patch(id, c.path, { state: c.state === 'ready' ? 'added' : 'seeded', listing });
      } catch (err) {
        patch(id, c.path, { state: 'failed', skip: String(err) });
      }
    }
    pumping = false;
    maybeFinish();
  };
  const cancel = () => {
    cancelled = true;
    stopCheck();
    useFileAdds.setState((s) => ({
      batches: s.batches.map((b) =>
        b.id === id ? { ...b, rows: b.rows.map((r) => (r.state === 'waiting' || r.state === 'checking' ? { ...r, state: 'skipped', skip: 'cancelled' } : r)) } : b,
      ),
    }));
    queue.length = 0;
    checkDone = true;
    maybeFinish();
  };

  useFileAdds.setState((s) => ({ batches: [...s.batches, { id, communityId, rows, cancel }] }));
  if (!todo.length) {
    checkDone = true;
    maybeFinish();
    return;
  }
  if (options.checked) {
    queue.push(...options.checked);
    checkDone = true;
    void pump();
    return;
  }
  for (const r of todo) patch(id, r.path, { state: 'checking' });
  stopCheck = checkFiles(communityId, todo.map((r) => r.path), {
    progress: (path, done, total) => patch(id, path, { progress: total ? done / total : 0 }),
    result: (c) => {
      queue.push(c);
      void pump();
    },
    done: () => {
      checkDone = true;
      maybeFinish();
    },
    failed: (err) => {
      toast(err);
      cancel();
    },
  });
}

async function addFilesManually(communityId: string): Promise<void> {
  const s = useShell.getState();
  if (s.community?.id !== communityId) return;
  const view = await filesApi.view(communityId);
  const filter = useFilesUi.getState().category;
  const target = view.categories.find((c) => c.id === filter && c.can_add && c.readable) ?? view.categories.find((c) => c.can_add && c.readable);
  if (target) await openAddFiles(s.community, view, target);
  else toast('You cannot add files to any category here.');
}

// Files dropped on the Files page: each to the category its type fits, with no dialog.
export async function dropOnFiles(paths: string[]): Promise<void> {
  const community = useShell.getState().community;
  if (!community) return;
  const filter = useFilesUi.getState().category;
  try {
    const plan = await filesApi.planDrop(community.id, paths, filter === 'all' ? null : filter);
    if (plan.no_categories) {
      toast('This server has no file categories yet. Someone who manages files needs to make one first.');
      return;
    }
    if (plan.folders.length) {
      const view = await filesApi.view(community.id);
      for (const folder of plan.folders) void openAddFolder(community, view, folder);
    }
    if (plan.created.length) toast(`Made ${plan.created.length === 1 ? 'a category' : 'categories'} for your files: ${plan.created.join(', ')}`, 'info');
    if (plan.items.length) {
      const view = await filesApi.view(community.id);
      startAdds(
        community.id,
        plan.items.map((i) => ({
          path: i.path,
          name: i.name,
          size: i.size,
          category: i.category,
          categoryName: i.category_name,
          state: i.skip ? 'skipped' : 'waiting',
          progress: 0,
          skip: i.skip,
          listing: null,
        })),
        { tagsFor: (row) => view.categories.find((c) => c.id === row.category)?.default_tags ?? [] },
      );
    }
  } catch (err) {
    toast(err);
  }
}

// The target shown while files are dragged over the Files page. It names the rule, once, since a drop asks nothing first.
export function FilesDropOverlay() {
  const over = useFileAdds((s) => s.dropOver);
  if (!over) return null;
  return (
    <div className="drop-overlay" data-files-drop aria-hidden="true">
      <div className="drop-box">
        <Upload aria-hidden="true" />
        <p className="drop-title">Drop to add to this server</p>
        <p className="muted">Each file goes to the category that fits its type, for every member to download. A folder asks first. Files go exactly as they are, photo details included. You can undo it.</p>
      </div>
    </div>
  );
}

const ROW_TEXT: Record<RowState, string> = {
  waiting: 'Waiting',
  checking: 'Checking…',
  listing: 'Adding…',
  added: 'Added',
  seeded: 'Seeding',
  skipped: 'Not added',
  failed: 'Failed',
};

// What is being added right now, per file. It sits over the page and never blocks it.
export function FileAddsTray() {
  const batches = useFileAdds((s) => s.batches);
  if (!batches.length) return null;
  return (
    <>
      {batches.map((b) => {
        const finished = b.rows.filter((r) => r.state === 'added' || r.state === 'seeded' || r.state === 'skipped' || r.state === 'failed').length;
        return (
          <section key={b.id} data-file-adds aria-label="Adding files" className="flex flex-col gap-1 rounded-lg border border-border-menu bg-ink px-s-3 py-s-2 text-sm text-text shadow-(--shadow-2)">
            <header className="flex items-center gap-s-2">
              <span className="min-w-0 flex-1 font-semibold text-text-strong">Adding {count(b.rows.length, 'file')} · {finished} done</span>
              <button type="button" aria-label="Cancel adding files" className="shrink-0 text-muted-foreground hover:text-text" onClick={b.cancel}>
                <X className="size-4" aria-hidden="true" />
              </button>
            </header>
            <ul role="list" className="m-0 flex max-h-40 list-none flex-col gap-0.5 overflow-y-auto p-0">
              {b.rows.map((r) => (
                <li key={r.path} data-add-row={r.state} className="flex items-center gap-s-2">
                  <span className="min-w-0 flex-1 truncate">{r.name}</span>
                  <span className="shrink-0 text-meta text-muted-foreground tabular-nums">
                    {ROW_TEXT[r.state]}
                    {r.state === 'checking' && r.progress ? ` ${Math.round(r.progress * 100)}%` : ''}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </>
  );
}

// Edits a listing's tags and description. A failure keeps the dialog and what was typed.
function EditListing({ communityId, listing, close }: { communityId: string; listing: ListingView; close: () => void }) {
  const [tags, setTags] = useState(listing.tags.map((t) => `#${t}`).join(' '));
  const [description, setDescription] = useState(listing.description);
  const [busy, setBusy] = useState(false);
  const save = async () => {
    setBusy(true);
    try {
      await filesApi.updateListing(communityId, listing.id, tags.split(/[\s,]+/).filter(Boolean), description);
      await reloadFiles(communityId);
      close();
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };
  return (
    <div data-edit-listing className="flex flex-col gap-s-4">
      <label className="flex flex-col gap-s-1">
        <span className="text-meta font-semibold text-muted-foreground uppercase">Tags</span>
        <Input aria-label="Tags" value={tags} onChange={(e) => setTags(e.target.value)} placeholder="#stable #windows" />
      </label>
      <label className="flex flex-col gap-s-1">
        <span className="text-meta font-semibold text-muted-foreground uppercase">Description</span>
        <Textarea aria-label="Description" value={description} maxLength={2000} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <div className="flex items-center justify-end gap-s-2">
        <Button variant="ghost" onClick={close}>Cancel</Button>
        <Button disabled={busy} onClick={() => void save()}>Save</Button>
      </div>
    </div>
  );
}

export function openEditListing(communityId: string, listing: ListingView): void {
  openDialog({ title: `Edit ${listing.name}`, content: (close) => <EditListing communityId={communityId} listing={listing} close={close} /> });
}

async function editListingById(communityId: string, id: string): Promise<void> {
  try {
    const listing = (await filesApi.view(communityId)).listings.find((l) => l.id === id);
    if (listing) openEditListing(communityId, listing);
  } catch (err) {
    toast(err);
  }
}
