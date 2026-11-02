// "Add a folder": what a chosen or dropped folder holds, what is left out and why, the category and tag, and a tick per
// file. Its files are seeded from where they are, with no copy. Nothing is truncated silently: every limit a folder
// exceeds is stated here. See docs/design/14-file-tracker.md, "Adding files".
import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { formatBytes } from '@/components/transfers/format';
import { openDialog } from '@/lib/dialog';
import { checkFiles, filesApi, tileStyle, type CandidateView, type FilesView, type FolderFile, type FolderScan } from '@/lib/files';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { startAdds, startFolderListing, type AddRow } from './file-adds';
import { useFilesUi } from './files-page';

// Why a file cannot be ticked at all. Everything else can, even what starts unticked.
const LEFT_OUT: Record<string, string> = {
  link: 'Link, not followed',
  too_large: 'Over 10 GB',
  already_listed: 'Already listed',
  type_not_allowed: 'Type not allowed here',
  unreadable: 'Can’t be read',
};
const FLAG_TEXT: Record<string, string> = {
  hidden: 'Hidden',
  system: 'System file',
  credentials: 'May hold a password or key',
};

// Picks a folder, or takes a dropped one, and opens the dialog at once. Nothing opens when the picker is cancelled.
export async function openAddFolder(community: CommunityView, view: FilesView, dropped?: string): Promise<void> {
  let folder = dropped ?? null;
  if (!folder) {
    try {
      folder = await filesApi.pickListingFolder();
    } catch (err) {
      toast(err);
      return;
    }
  }
  if (!folder) return;
  const chosen = folder;
  openDialog({
    title: `Add folder ${chosen.split(/[\\/]/).filter(Boolean).pop() ?? chosen}`,
    sub: 'Its files stay where they are and are seeded from this folder, with no copy.',
    wide: true,
    content: (close) => <AddFolder community={community} view={view} folder={chosen} close={close} />,
  });
}

function AddFolder({ community, view, folder, close }: { community: CommunityView; view: FilesView; folder: string; close: () => void }) {
  const [scan, setScan] = useState<FolderScan | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [categoryId, setCategoryId] = useState('');
  const [tag, setTag] = useState('');
  const [on, setOn] = useState<Record<string, boolean>>({});
  const [checked, setChecked] = useState<Record<string, CandidateView>>({});
  const [separate, setSeparate] = useState(false);
  const targets = view.categories.filter((c) => c.can_add && c.readable);
  const category = targets.find((c) => c.id === categoryId);

  useEffect(() => {
    let live = true;
    const filter = useFilesUi.getState().category;
    filesApi.scanListingFolder(community.id, folder, filter === 'all' ? null : filter).then(
      (found) => {
        if (!live) return;
        setScan(found);
        setCategoryId(found.category ?? targets[0]?.id ?? '');
        setTag(found.tag);
        setOn(Object.fromEntries(found.files.map((f) => [f.path, f.flag === ''])));
      },
      (err) => live && setFailed(String(err)),
    );
    return () => {
      live = false;
    };
    // The scan runs once for the folder the dialog opened on.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [community.id, folder]);

  // Every file that could be ticked is checked, so ticking one later does not wait.
  useEffect(() => {
    if (!scan) return undefined;
    const paths = scan.files.filter((f) => f.flag !== 'link' && f.flag !== 'too_large').map((f) => f.path);
    if (!paths.length) return undefined;
    return checkFiles(community.id, paths, {
      result: (c) => setChecked((prev) => ({ ...prev, [c.path]: c })),
      failed: (err) => toast(err),
    });
  }, [community.id, scan]);

  const leftOut = (f: FolderFile): string | null => {
    if (f.flag === 'link' || f.flag === 'too_large') return f.flag;
    if (category?.types.length && !category.types.includes(f.kind)) return 'type_not_allowed';
    const state = checked[f.path]?.state;
    return state && state !== 'ready' ? state : null;
  };
  const files = scan?.files ?? [];
  const ticked = files.filter((f) => on[f.path] && !leftOut(f));
  const checking = ticked.filter((f) => !checked[f.path]).length;
  const size = ticked.reduce((n, f) => n + f.size, 0);
  const notes = useMemo(() => {
    if (!scan) return [];
    const out: string[] = [];
    if (scan.over_limit) out.push(`A folder adds at most ${scan.max_files} files at once, so ${scan.over_limit} more ${scan.over_limit === 1 ? 'file is' : 'files are'} left out.`);
    if (scan.too_deep) out.push(`${scan.too_deep} ${scan.too_deep === 1 ? 'folder is' : 'folders are'} more than ${scan.max_depth} levels down and ${scan.too_deep === 1 ? 'was' : 'were'} not opened.`);
    if (scan.flagged_more) out.push(`${scan.flagged_more} more hidden or system ${scan.flagged_more === 1 ? 'file is' : 'files are'} left out.`);
    if (scan.stopped_early) out.push('This folder is too big to look through to the end, so more files may be in it.');
    return out;
  }, [scan]);
  const flagged = files.some((f) => f.flag === 'hidden' || f.flag === 'system') || (scan?.hidden_dirs.length ?? 0) > 0;
  const secrets = files.some((f) => f.flag === 'credentials');

  const add = () => {
    if (!category || !scan) return;
    const rows: AddRow[] = ticked.map((f) => ({
      path: f.path,
      name: f.name,
      size: f.size,
      category: category.id,
      categoryName: category.name,
      state: 'waiting',
      progress: 0,
      skip: null,
      listing: null,
    }));
    const tags = [...category.default_tags, ...tag.split(/[\s,]+/).filter(Boolean)];
    if (separate) {
      startAdds(community.id, rows, { tagsFor: () => tags, inPlace: true, checked: ticked.map((f) => checked[f.path]) });
    } else {
      startFolderListing(community.id, scan.name, category.name, () => filesApi.listFolder(community.id, category.id, scan.root, ticked.map((f) => f.relative), tags, '', 'forever'));
    }
    close();
  };

  if (failed) return <p className="m-0 text-body text-danger-text">{failed}</p>;
  if (!scan) return <p className="m-0 text-body text-muted-foreground" data-add-folder="scanning">Looking through the folder…</p>;
  return (
    <div data-add-folder="ready" className="flex flex-col gap-s-4">
      <p className="m-0 text-ui text-text-strong" data-folder-count>
        {ticked.length} {ticked.length === 1 ? 'file' : 'files'} · {formatBytes(size)}
        {checking ? ` · checking ${checking}…` : ''}
      </p>
      {notes.map((n) => (
        <p key={n} data-folder-limit className="m-0 rounded-md border border-border bg-panel px-s-3 py-s-2 text-sm text-text">{n}</p>
      ))}
      {flagged ? (
        <p data-folder-hidden className="m-0 text-sm text-muted-foreground">
          Hidden and system files start unticked, such as dotfiles, desktop.ini and Thumbs.db.
          {scan.hidden_dirs.length ? ` Hidden folders are not opened: ${scan.hidden_dirs.slice(0, 5).join(', ')}${scan.hidden_dirs.length > 5 ? ` and ${scan.hidden_dirs.length - 5} more` : ''}.` : ''}
        </p>
      ) : null}
      {secrets ? (
        <p data-folder-secrets className="m-0 rounded-md border border-danger-text bg-danger-tint px-s-3 py-s-2 text-sm text-text-strong">
          Some files look like passwords or keys, and start unticked. Every member could download a file you tick.
        </p>
      ) : null}
      <ul role="list" className="m-0 flex max-h-[320px] list-none flex-col gap-s-1 overflow-y-auto p-0">
        {files.map((f) => {
          const out = leftOut(f);
          const note = out ? LEFT_OUT[out] : !checked[f.path] && f.flag === '' ? 'Checking…' : FLAG_TEXT[f.flag] ?? 'Ready';
          return (
            <li key={f.path} data-folder-row={out ?? (f.flag || 'ok')} className="rounded-lg border border-border bg-panel px-s-3 py-s-1">
              <label className="flex items-center gap-s-3">
                <input type="checkbox" checked={!!on[f.path] && !out} disabled={!!out} onChange={() => setOn({ ...on, [f.path]: !on[f.path] })} aria-label={f.relative} />
                <span className="grid h-6 w-8 shrink-0 place-items-center rounded text-[9px] font-bold text-white" style={tileStyle(f.kind)}>{f.ext || 'FILE'}</span>
                <span className="min-w-0 flex-1 truncate text-ui text-text-strong" title={f.relative}>{f.relative}</span>
                <span className="text-sm text-muted-foreground tabular-nums">{formatBytes(f.size)}</span>
                <span className={cn('w-44 text-right text-sm font-semibold', out || f.flag === 'credentials' ? 'text-danger-text' : 'text-muted-foreground')}>{note}</span>
              </label>
            </li>
          );
        })}
        {!files.length ? <li className="text-sm text-muted-foreground">No files to add in this folder.</li> : null}
      </ul>
      <div className="grid grid-cols-2 gap-s-3">
        <label className="flex flex-col gap-s-1">
          <span className="text-meta font-semibold text-muted-foreground uppercase">Category</span>
          <select aria-label="Category" value={categoryId} onChange={(e) => setCategoryId(e.target.value)} className="h-9 rounded-md border border-input-border bg-input px-s-2 text-ui text-text">
            {targets.map((c) => (
              <option key={c.id} value={c.id}>{c.name}</option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-s-1">
          <span className="text-meta font-semibold text-muted-foreground uppercase">Tag</span>
          <Input aria-label="Tag" value={tag} onChange={(e) => setTag(e.target.value)} />
        </label>
      </div>
      <fieldset className="m-0 flex flex-col gap-1 border-0 p-0 text-ui text-text-strong">
        <label className="flex items-center gap-s-2">
          <input type="radio" name="folder-as" checked={!separate} onChange={() => setSeparate(false)} />
          List as one folder <span className="text-sm text-muted-foreground">Members see what is inside and download it whole.</span>
        </label>
        <label className="flex items-center gap-s-2">
          <input type="radio" name="folder-as" checked={separate} onChange={() => setSeparate(true)} />
          List each file separately
        </label>
      </fieldset>
      <div className="flex items-center gap-s-2">
        <span className="mr-auto text-sm text-muted-foreground">Every member can download these. Files go exactly as they are, photo details included. Keep the folder where it is, or they stop being seeded.</span>
        <Button variant="ghost" onClick={close}>Cancel</Button>
        <Button disabled={!category || checking > 0 || ticked.length === 0} onClick={add}>
          {checking ? `Checking ${checking}…` : separate ? `Add ${ticked.length} ${ticked.length === 1 ? 'file' : 'files'}` : `List folder · ${ticked.length} ${ticked.length === 1 ? 'file' : 'files'}`}
        </Button>
      </div>
    </div>
  );
}
