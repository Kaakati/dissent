// "Add files to a category": the picked (or scanned) files with their state, then tags, category, description and
// whether to seed. A file already listed with the same fingerprint seeds that listing instead of making a second one.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { formatBytes } from '@/components/transfers/format';
import { openDialog } from '@/lib/dialog';
import { filesApi, reloadFiles, tileStyle, type CandidateView, type CategoryView, type FilesView } from '@/lib/files';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';

const STATE_TEXT: Record<CandidateView['state'], string> = {
  ready: 'Ready',
  already_listed: 'Already listed',
  type_not_allowed: 'Type not allowed here',
  too_large: 'Over 10 GB',
  unreadable: 'Can’t be read',
};
const STATE_COLOR: Record<CandidateView['state'], string> = {
  ready: 'text-success-text',
  already_listed: 'text-info',
  type_not_allowed: 'text-danger-text',
  too_large: 'text-danger-text',
  unreadable: 'text-danger-text',
};
const addable = (c: CandidateView) => c.state === 'ready' || c.state === 'already_listed';

// Picks files, or scans the category's linked folder, and opens the dialog for what was found. Nothing opens when the
// picker is cancelled; a scan that finds nothing new says so.
export async function openAddFiles(community: CommunityView, view: FilesView, category: CategoryView, scan = false): Promise<void> {
  let found: CandidateView[];
  try {
    found = scan ? await filesApi.scanFolder(community.id, category.id) : await filesApi.pickFiles(community.id, category.id);
  } catch (err) {
    toast(err);
    return;
  }
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

function AddFiles({ community, view, category, found, close }: { community: CommunityView; view: FilesView; category: CategoryView; found: CandidateView[]; close: () => void }) {
  const [on, setOn] = useState<Record<string, boolean>>(() => Object.fromEntries(found.map((f) => [f.path, addable(f)])));
  const [tags, setTags] = useState(category.default_tags.map((t) => `#${t}`).join(' '));
  const [description, setDescription] = useState('');
  const [categoryId, setCategoryId] = useState(category.id);
  const [seed, setSeed] = useState(true);
  const [busy, setBusy] = useState(false);
  const chosen = found.filter((f) => on[f.path] && addable(f));
  const fresh = chosen.filter((f) => f.state === 'ready').length;
  const listed = chosen.length - fresh;
  const cta = chosen.length === 0 ? 'Add files' : `Add ${fresh} ${fresh === 1 ? 'file' : 'files'}${listed ? ` · seed ${listed} listed` : ''}`;
  const targets = view.categories.filter((c) => c.can_add && c.readable);

  const add = async () => {
    setBusy(true);
    try {
      const tagList = tags.split(/[\s,]+/).filter(Boolean);
      // Seeding is how the file reaches others, so "off" seeds only until one member has a full copy.
      await filesApi.list(community.id, categoryId, chosen.map((f) => ({ path: f.path, tags: tagList, description })), seed ? 'forever' : 'ratio:1000');
      await reloadFiles(community.id);
      toast(`${chosen.length} ${chosen.length === 1 ? 'file' : 'files'} added`, 'info');
      close();
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };

  return (
    <div data-add-files className="flex flex-col gap-s-4">
      <ul role="list" className="m-0 flex list-none flex-col gap-s-1 p-0">
        {found.map((f) => (
          <li key={f.path} className="flex flex-col gap-1 rounded-lg border border-border bg-panel px-s-3 py-s-2">
            <label className="flex items-center gap-s-3">
              <input type="checkbox" checked={!!on[f.path]} disabled={!addable(f)} onChange={() => setOn({ ...on, [f.path]: !on[f.path] })} />
              <span className="grid h-7 w-9 shrink-0 place-items-center rounded text-[10px] font-bold text-white" style={tileStyle(f.kind)}>{f.ext || 'FILE'}</span>
              <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{f.name}</span>
              <span className="text-sm text-muted-foreground tabular-nums">{formatBytes(f.size)}</span>
              <span className={cn('w-40 text-right text-sm font-semibold', STATE_COLOR[f.state])} data-state={f.state}>{STATE_TEXT[f.state]}</span>
            </label>
            {f.state === 'already_listed' && on[f.path] ? (
              <p className="m-0 pl-[76px] text-sm text-muted-foreground">
                Already listed by {f.listed_by} with the same fingerprint. You’ll seed that listing instead of adding a second one, and your uploads still count for you.
              </p>
            ) : null}
          </li>
        ))}
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
          <span>Seed after adding</span>
          <span className="text-sm text-muted-foreground">Members can download as soon as the listing is up.</span>
        </span>
      </label>
      <div className="flex items-center gap-s-2">
        <span className="mr-auto text-sm text-muted-foreground">The listing is signed by you and shared with members only. Files go exactly as they are, photo details included, so their fingerprint matches.</span>
        <Button variant="ghost" onClick={close}>Cancel</Button>
        <Button disabled={busy || chosen.length === 0} onClick={() => void add()}>{cta}</Button>
      </div>
    </div>
  );
}
