// "Add files to a category": the picked (or scanned) files with their state, then tags, category, description and
// whether to seed. A file already listed with the same fingerprint seeds that listing instead of making a second one.
// The dialog opens as soon as the files are chosen. It knows their names, sizes and types by then; each row's
// fingerprint is checked in the background, and closing the dialog stops that.
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { formatBytes } from '@/components/transfers/format';
import { openDialog } from '@/lib/dialog';
import { checkFiles, filesApi, reloadFiles, tileStyle, type CandidateState, type CandidateView, type CategoryView, type FilesView, type PickedView } from '@/lib/files';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';

type RowState = CandidateState | 'checking';

const STATE_TEXT: Record<RowState, string> = {
  checking: 'Checking…',
  ready: 'Ready',
  already_listed: 'Already listed',
  type_not_allowed: 'Type not allowed here',
  too_large: 'Over 10 GB',
  unreadable: 'Can’t be read',
};
const STATE_COLOR: Record<RowState, string> = {
  checking: 'text-muted-foreground',
  ready: 'text-success-text',
  already_listed: 'text-info',
  type_not_allowed: 'text-danger-text',
  too_large: 'text-danger-text',
  unreadable: 'text-danger-text',
};

// Picks files, or finds what is new in the category's linked folder, and opens the dialog for what was found. Nothing
// opens when the picker is cancelled; a scan that finds nothing new says so.
export async function openAddFiles(community: CommunityView, view: FilesView, category: CategoryView, scan = false): Promise<void> {
  let found: PickedView[];
  try {
    found = scan ? await filesApi.scanFolder(community.id, category.id) : await filesApi.pickFiles();
  } catch (err) {
    toast(err);
    return;
  }
  found = found.filter((f) => !f.is_dir);
  if (!found.length) {
    if (scan) toast(`Nothing new in ${category.folder}`, 'info');
    return;
  }
  openDialog({
    title: `Add files to ${category.name}`,
    sub: scan
      ? `Found in ${category.folder} since the last scan. They stay where they are and are seeded from that folder.`
      : 'The files are encrypted for this server’s members and seeded from this computer.',
    wide: true,
    content: (close) => <AddFiles community={community} view={view} category={category} found={found} close={close} />,
  });
}

function AddFiles({ community, view, category, found, close }: { community: CommunityView; view: FilesView; category: CategoryView; found: PickedView[]; close: () => void }) {
  const [on, setOn] = useState<Record<string, boolean>>(() => Object.fromEntries(found.map((f) => [f.path, true])));
  const [tags, setTags] = useState(category.default_tags.map((t) => `#${t}`).join(' '));
  const [description, setDescription] = useState('');
  const [categoryId, setCategoryId] = useState(category.id);
  const [seed, setSeed] = useState(true);
  const [busy, setBusy] = useState(false);
  const [checked, setChecked] = useState<Record<string, CandidateView>>({});
  const [progress, setProgress] = useState<Record<string, number>>({});
  const targets = view.categories.filter((c) => c.can_add && c.readable);
  const types = view.categories.find((c) => c.id === categoryId)?.types ?? [];

  useEffect(
    () =>
      checkFiles(community.id, found.map((f) => f.path), {
        progress: (path, done, total) => setProgress((p) => ({ ...p, [path]: total ? done / total : 0 })),
        result: (candidate) => setChecked((c) => ({ ...c, [candidate.path]: candidate })),
        failed: (err) => toast(err),
      }),
    [community.id, found],
  );

  // A file's type is known from the start, and the category can change under it, so that is never waited for.
  const stateOf = (f: PickedView): RowState => {
    if (types.length && !types.includes(f.kind)) return 'type_not_allowed';
    return checked[f.path]?.state ?? 'checking';
  };
  const addable = (f: PickedView) => {
    const state = stateOf(f);
    return state === 'ready' || state === 'already_listed' || state === 'checking';
  };
  const ticked = found.filter((f) => on[f.path] && addable(f));
  const checking = ticked.filter((f) => stateOf(f) === 'checking').length;
  const fresh = ticked.filter((f) => stateOf(f) === 'ready').length;
  const listed = ticked.length - fresh - checking;
  const cta = checking
    ? `Checking ${checking} ${checking === 1 ? 'file' : 'files'}…`
    : ticked.length === 0
      ? 'Add files'
      : `Add ${fresh} ${fresh === 1 ? 'file' : 'files'}${listed ? ` · seed ${listed} listed` : ''}`;

  const add = async () => {
    setBusy(true);
    try {
      const tagList = tags.split(/[\s,]+/).filter(Boolean);
      // Seeding is how the file reaches others, so "off" seeds only until one member has a full copy.
      await filesApi.list(community.id, categoryId, ticked.map((f) => ({ path: f.path, tags: tagList, description })), seed ? 'forever' : 'ratio:1000');
      await reloadFiles(community.id);
      toast(`${ticked.length} ${ticked.length === 1 ? 'file' : 'files'} added`, 'info');
      close();
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };

  return (
    <div data-add-files className="flex flex-col gap-s-4">
      <ul role="list" className="m-0 flex list-none flex-col gap-s-1 p-0">
        {found.map((f) => {
          const state = stateOf(f);
          const result = checked[f.path];
          return (
            <li key={f.path} className="flex flex-col gap-1 rounded-lg border border-border bg-panel px-s-3 py-s-2">
              <label className="flex items-center gap-s-3">
                <input type="checkbox" checked={!!on[f.path] && addable(f)} disabled={!addable(f)} onChange={() => setOn({ ...on, [f.path]: !on[f.path] })} />
                <span className="grid h-7 w-9 shrink-0 place-items-center rounded text-[10px] font-bold text-white" style={tileStyle(f.kind)}>{f.ext || 'FILE'}</span>
                <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{f.name}</span>
                <span className="text-sm text-muted-foreground tabular-nums">{formatBytes(f.size)}</span>
                <span className={cn('w-40 text-right text-sm font-semibold tabular-nums', STATE_COLOR[state])} data-state={state}>
                  {STATE_TEXT[state]}
                  {state === 'checking' && progress[f.path] ? ` ${Math.round(progress[f.path] * 100)}%` : ''}
                </span>
              </label>
              {state === 'already_listed' && on[f.path] ? (
                <p className="m-0 pl-[76px] text-sm text-muted-foreground">
                  Already listed by {result?.listed_by} with the same fingerprint. You’ll seed that listing instead of adding a second one, and your uploads still count for you.
                </p>
              ) : null}
            </li>
          );
        })}
      </ul>
      <label className="flex flex-col gap-s-1">
        <span className="text-meta font-semibold text-muted-foreground uppercase">Tags</span>
        <Input aria-label="Tags" value={tags} onChange={(e) => setTags(e.target.value)} placeholder="#stable #windows" />
      </label>
      <label className="flex flex-col gap-s-1">
        <span className="text-meta font-semibold text-muted-foreground uppercase">Category</span>
        <select aria-label="Category" value={categoryId} onChange={(e) => setCategoryId(e.target.value)} className="h-9 rounded-md border border-input-border bg-input px-s-2 text-ui text-text">
          {targets.map((c) => (
            <option key={c.id} value={c.id}>{c.name}</option>
          ))}
        </select>
      </label>
      <label className="flex flex-col gap-s-1">
        <span className="text-meta font-semibold text-muted-foreground uppercase">Description</span>
        <Textarea aria-label="Description" value={description} maxLength={2000} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <label className="flex items-center gap-s-3 text-ui text-text-strong">
        <input type="checkbox" checked={seed} onChange={() => setSeed(!seed)} />
        <span className="flex flex-col">
          <span>Keep seeding after one full copy has gone out</span>
          <span className="text-sm text-muted-foreground">Off: stops once members have received one full copy from you.</span>
        </span>
      </label>
      <div className="flex items-center gap-s-2">
        <span className="mr-auto text-sm text-muted-foreground">The listing is signed by you and shared with members only. Files go exactly as they are, photo details included, so their fingerprint matches.</span>
        <Button variant="ghost" onClick={close}>Cancel</Button>
        <Button disabled={busy || checking > 0 || ticked.length === 0} onClick={() => void add()}>{cta}</Button>
      </div>
    </div>
  );
}
