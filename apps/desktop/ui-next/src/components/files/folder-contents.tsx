// A folder listing's contents: the tree of its files, frozen when it was listed, with each file's type and size, who
// holds it, totals, and a note when any file can run code. It reads well with hundreds of files: the tree scrolls, folders
// collapse, and a search narrows it. Also the folder's download, file by file. See docs/design/14-file-tracker.md.
import { useEffect, useMemo, useState } from 'react';
import { ChevronDown, ChevronRight, CircleAlert, Folder } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { formatBytes } from '@/components/transfers/format';
import { openDialog } from '@/lib/dialog';
import { filesApi, tileStyle, type FolderContentsView, type FolderEntryView, type FolderFileProgress, type ListingView, type LiveSwarm } from '@/lib/files';
import type { Id } from '@/lib/types';
import { cn } from '@/lib/utils';

interface Node {
  name: string;
  path: string;
  dirs: Map<string, Node>;
  files: FolderEntryView[];
  count: number;
  size: number;
}

const newNode = (name: string, path: string): Node => ({ name, path, dirs: new Map(), files: [], count: 0, size: 0 });

// The tree of `entries`, each folder with the files and bytes below it.
export function folderTree(entries: FolderEntryView[]): Node {
  const root = newNode('', '');
  for (const e of entries) {
    const parts = e.path.split('/');
    let at = root;
    at.count += 1;
    at.size += e.size;
    for (const part of parts.slice(0, -1)) {
      const path = at.path ? `${at.path}/${part}` : part;
      if (!at.dirs.has(part)) at.dirs.set(part, newNode(part, path));
      at = at.dirs.get(part) as Node;
      at.count += 1;
      at.size += e.size;
    }
    at.files.push(e);
  }
  return root;
}

const plural = (n: number, one: string) => `${n.toLocaleString()} ${n === 1 ? one : `${one}s`}`;

function Tree({ node, depth, open, toggle }: { node: Node; depth: number; open: Set<string>; toggle: (path: string) => void }) {
  const dirs = [...node.dirs.values()].sort((a, b) => a.name.localeCompare(b.name));
  const files = [...node.files].sort((a, b) => a.name.localeCompare(b.name));
  return (
    <>
      {dirs.map((d) => {
        const shown = open.has(d.path);
        return (
          <li key={`d:${d.path}`} role="treeitem" aria-expanded={shown} aria-level={depth + 1} data-folder-dir={d.path}>
            <button type="button" className="flex h-7 w-full items-center gap-s-1 rounded px-s-1 text-left text-ui hover:bg-hover" style={{ paddingLeft: depth * 16 + 4 }} onClick={() => toggle(d.path)}>
              {shown ? <ChevronDown className="size-4 shrink-0" aria-hidden /> : <ChevronRight className="size-4 shrink-0" aria-hidden />}
              <Folder className="size-4 shrink-0 text-muted-foreground" aria-hidden />
              <span className="min-w-0 flex-1 truncate text-text-strong">{d.name}</span>
              <span className="shrink-0 text-meta text-muted-foreground tabular-nums">{plural(d.count, 'file')} · {formatBytes(d.size)}</span>
            </button>
            {shown ? (
              <ul role="group" className="m-0 list-none p-0">
                <Tree node={d} depth={depth + 1} open={open} toggle={toggle} />
              </ul>
            ) : null}
          </li>
        );
      })}
      {files.map((f) => (
        <li key={`f:${f.path}`} role="treeitem" aria-level={depth + 1} data-folder-file={f.path} className="flex h-7 items-center gap-s-2 px-s-1 text-ui" style={{ paddingLeft: depth * 16 + 24 }}>
          <span className="grid h-5 w-7 shrink-0 place-items-center rounded text-[8px] font-bold text-white" style={tileStyle(f.kind)}>{f.ext || 'FILE'}</span>
          <span className="min-w-0 flex-1 truncate text-text">{f.name}</span>
          {f.danger ? <CircleAlert className="size-4 shrink-0 text-danger-text" aria-label="Can run code" /> : null}
          {f.holders === 0 ? <span className="shrink-0 text-meta text-warn">No one online has it</span> : null}
          <span className="w-16 shrink-0 text-right text-meta text-muted-foreground tabular-nums">{formatBytes(f.size)}</span>
        </li>
      ))}
    </>
  );
}

// The contents panel. Loads the folder's contents, fetching them from a member when this device does not have them yet.
export function FolderContents({ communityId, listing, compact = false }: { communityId: Id; listing: ListingView; compact?: boolean }) {
  const [view, setView] = useState<FolderContentsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState<Set<string>>(new Set());
  useEffect(() => {
    let live = true;
    filesApi.folderContents(communityId, listing.id).then(
      (v) => {
        if (!live) return;
        setView(v);
        // A small folder opens all the way; a big one shows its top level.
        if (v.entries.length <= 40) setOpen(new Set(v.entries.flatMap((e) => e.path.split('/').slice(0, -1).map((_, i, a) => a.slice(0, i + 1).join('/')))));
      },
      (err: unknown) => live && setError(String(err)),
    );
    return () => {
      live = false;
    };
  }, [communityId, listing.id]);
  const q = query.trim().toLowerCase();
  const shown = useMemo(() => (view ? view.entries.filter((e) => !q || e.path.toLowerCase().includes(q)) : []), [view, q]);
  const tree = useMemo(() => folderTree(shown), [shown]);
  // While searching, every folder on the way to a match is open.
  const openNow = useMemo(
    () => (q ? new Set(shown.flatMap((e) => e.path.split('/').slice(0, -1).map((_, i, a) => a.slice(0, i + 1).join('/')))) : open),
    [q, shown, open],
  );
  if (error) return <p className="m-0 text-sm text-muted-foreground">The folder’s contents could not be read: {error.replace(/^invalid: /, '')}</p>;
  if (!view) return <p className="m-0 text-sm text-muted-foreground" data-folder-loading>Getting the folder’s contents…</p>;
  const toggle = (path: string) => setOpen((s) => {
    const next = new Set(s);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    return next;
  });
  return (
    <section aria-label="Folder contents" data-folder-contents className="flex min-h-0 flex-col gap-s-2">
      <p className="m-0 text-sm text-text" data-folder-totals>
        {plural(view.files, 'file')} · {formatBytes(view.size)}
        {view.unavailable ? ` · ${plural(view.unavailable, 'file')} no one online has right now` : ''}
      </p>
      {view.dangerous ? (
        <p data-folder-danger role="note" className="m-0 flex items-center gap-s-2 rounded-md border border-danger-text bg-danger-tint px-s-3 py-s-2 text-sm text-text-strong">
          <CircleAlert className="size-4 shrink-0 text-danger-text" aria-hidden />
          {view.dangerous === 1 ? '1 file in this folder can run code.' : `${view.dangerous} files in this folder can run code.`} Open them only if you trust {listing.lister_name}.
        </p>
      ) : null}
      {view.entries.length > 10 ? <Input aria-label="Search this folder" placeholder="Search this folder" value={query} onChange={(e) => setQuery(e.target.value)} /> : null}
      <ul role="tree" aria-label={`${view.name} contents`} className={cn('m-0 list-none overflow-y-auto rounded-md border border-border p-s-1', compact ? 'max-h-[360px]' : 'max-h-[480px]')}>
        <Tree node={tree} depth={0} open={openNow} toggle={toggle} />
        {!shown.length ? <li className="p-s-2 text-sm text-muted-foreground">No file matches.</li> : null}
      </ul>
    </section>
  );
}

export function openFolderContents(communityId: Id, listing: ListingView): void {
  openDialog({ title: listing.name, sub: 'What this folder held when it was listed.', wide: true, content: () => <FolderContents communityId={communityId} listing={listing} compact /> });
}

const rateOf = (bps: number) => `${formatBytes(bps)}/s`;

// A folder download: its speed, from this device's own counts, and each file as it comes in.
export function FolderDownload({ communityId, listing }: { communityId: Id; listing: ListingView }) {
  const [live, setLive] = useState<LiveSwarm | null>(null);
  const [files, setFiles] = useState<FolderFileProgress[]>([]);
  const downloading = listing.local === 'downloading';
  useEffect(() => {
    let on = true;
    const read = () => {
      filesApi.live(communityId, listing.id).then((n) => on && setLive(n), () => {});
      if (downloading) filesApi.folderFiles(listing.id).then((f) => on && setFiles(f), () => {});
    };
    read();
    const timer = setInterval(read, 1000);
    return () => {
      on = false;
      clearInterval(timer);
    };
  }, [communityId, listing.id, downloading]);
  if (!downloading && !live?.active) return null;
  const done = files.filter((f) => f.done).length;
  return (
    <section aria-label="Folder download" data-folder-download className="flex max-w-[760px] flex-col gap-s-1 rounded-lg border border-border bg-panel p-s-3 text-sm text-text">
      <p className="m-0 font-semibold text-text-strong" data-folder-progress>
        {downloading ? `Downloading · ${Math.round(listing.progress * 100)}% · ${done} of ${listing.folder?.files ?? files.length} files` : 'Sharing this folder'}
        {live ? ` · ↓ ${rateOf(live.down_bps)} · ↑ ${rateOf(live.up_bps)}` : ''}
      </p>
      {files.length ? (
        <ul role="list" className="m-0 max-h-40 list-none overflow-y-auto p-0">
          {files.map((f) => (
            <li key={f.path} data-folder-file-progress={f.done ? 'done' : 'waiting'} className="flex justify-between gap-s-2 text-meta">
              <span className="min-w-0 truncate">{f.path}</span>
              <span className="shrink-0 text-muted-foreground">{f.done ? 'Done' : 'Waiting'}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
