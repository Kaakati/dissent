// The server's Files area (the tracker): Browse with category, type, tag and health filters and a sortable table, the
// file page, My seeding and Uploads. Opened from the server sidebar. See docs/design/14-file-tracker.md.
import { useEffect, useMemo, useRef, useState } from 'react';
import { create } from 'zustand';
import { ArrowLeft, Download, FolderOpen, FolderPlus, Link2, Lock, Pencil, Plus, Search, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Tip } from '@/components/ui/tip';
import { formatBytes } from '@/components/transfers/format';
import { confirm } from '@/lib/dialog';
import { filesApi, formatLevel, reloadFiles, tileStyle, useFilesOf, type CategoryView, type FilesView, type ListingView } from '@/lib/files';
import { downloadWithin, OLDER_VERSIONS_NOTE, RELEASED_LIMIT } from '@/lib/limits';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { DangerNote } from '@/components/chat/attachments';
import '@/components/chat/chat.css';
import { openAddFiles } from './add-files-dialog';
import { openAddFolder } from './add-folder-dialog';
import { openCategoryEditor } from '@/components/server-settings/files-section';
import { FilesDropOverlay, openEditListing } from './file-adds';
import { SeedingPanel } from './seeding-panel';
import { FolderContents, FolderDownload, openFolderContents } from './folder-contents';
import { SwarmPanel } from './swarm-panel';

type Tab = 'browse' | 'seeding' | 'uploads';
type Health = 'all' | 'well' | 'needs' | 'dead';
type SortKey = 'name' | 'category' | 'size' | 'seeders' | 'leechers' | 'completed' | 'added' | 'uploader';

interface FilesUi {
  tab: Tab;
  category: string; // 'all' or a category id
  hiddenTypes: string[];
  tag: string;
  health: Health;
  sort: SortKey;
  desc: boolean;
  query: string;
  open: string | null; // the listing whose page is shown
  page: number; // 1 is the first
  pageSize: number; // remembered on this device
}

export const PAGE_SIZES = [15, 25, 50, 75];
const PAGE_SIZE_KEY = 'dissent.files.pageSize';

function savedPageSize(): number {
  try {
    const n = Number(window.localStorage.getItem(PAGE_SIZE_KEY));
    return PAGE_SIZES.includes(n) ? n : 25;
  } catch {
    return 25;
  }
}

function setPageSize(n: number): void {
  try {
    window.localStorage.setItem(PAGE_SIZE_KEY, String(n));
  } catch {
    // Kept for this run only.
  }
  useFilesUi.setState({ pageSize: n });
}

export const useFilesUi = create<FilesUi>(() => ({
  tab: 'browse',
  category: 'all',
  hiddenTypes: [],
  tag: '',
  health: 'all',
  sort: 'seeders',
  desc: true,
  query: '',
  open: null,
  page: 1,
  pageSize: savedPageSize(),
}));

// What the list shows changed, so it starts again from the first page. A listing arriving changes none of this.
const listKey = (s: FilesUi) => JSON.stringify([s.tab, s.category, s.hiddenTypes, s.tag, s.health, s.sort, s.desc, s.query, s.pageSize]);
let lastListKey = listKey(useFilesUi.getState());
useFilesUi.subscribe((s) => {
  const key = listKey(s);
  if (key === lastListKey) return;
  lastListKey = key;
  if (s.page !== 1) useFilesUi.setState({ page: 1 });
});

const healthOf = (l: ListingView): Health => (l.seeders >= 5 ? 'well' : l.seeders > 0 ? 'needs' : 'dead');
const SEED_COLOR: Record<Health, string> = { all: '', well: 'text-success-text', needs: 'text-warn', dead: 'text-danger-text' };
const dateOf = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });

function useCommunityFiles(): { community: CommunityView | null; view: FilesView | null } {
  const community = useShell((s) => (s.route.kind === 'server' ? s.community : null));
  const transfers = useShell((s) => s.transfers);
  // Only a change of state reloads the whole view: a running download's figures come from its own row.
  const transferStates = transfers.map((t) => `${t.id}:${t.state}`).join();
  const view = useFilesOf(community?.id ?? null, community, transferStates);
  return { community, view };
}

export function FilesTabs() {
  const { view } = useCommunityFiles();
  const tab = useFilesUi((s) => s.tab);
  const counts: Record<Tab, number> = {
    browse: view?.listings.length ?? 0,
    seeding: view?.listings.filter((l) => l.local === 'seeding' || l.local === 'paused').length ?? 0,
    uploads: view?.listings.filter((l) => l.mine).length ?? 0,
  };
  const tabs: { key: Tab; label: string }[] = [
    { key: 'browse', label: 'Browse' },
    { key: 'seeding', label: 'My seeding' },
    { key: 'uploads', label: 'Uploads' },
  ];
  return (
    <div role="tablist" aria-label="Files" className="flex gap-s-2">
      {tabs.map((t) => (
        <button
          key={t.key}
          type="button"
          role="tab"
          aria-selected={tab === t.key}
          onClick={() => useFilesUi.setState({ tab: t.key, open: null })}
          className={cn(
            'inline-flex h-8 items-center gap-s-2 rounded-md px-s-3 text-ui font-semibold transition-colors',
            tab === t.key ? 'bg-selected text-text-strong' : 'text-muted-foreground hover:bg-hover hover:text-text',
          )}
        >
          <span>{t.label}</span>
          <span className="min-w-5 rounded-full bg-elevated px-1.5 text-center text-meta leading-[18px] font-semibold text-text tabular-nums">
            {counts[t.key]}
          </span>
        </button>
      ))}
    </div>
  );
}

export function FilesBody() {
  const { community, view } = useCommunityFiles();
  const ui = useFilesUi();
  if (!community) return null;
  if (!view) return <p className="p-s-6 text-body text-muted-foreground">Loading files…</p>;
  const open = ui.open ? view.listings.find((l) => l.id === ui.open) : undefined;
  return (
    <div className="relative grid min-h-0">
      {open ? (
        <FilePage community={community} view={view} listing={open} />
      ) : ui.tab === 'seeding' ? (
        <SeedingPanel communityId={community.id} trigger={view} />
      ) : (
        <div data-files className="grid min-h-0 grid-cols-[220px_minmax(0,1fr)_260px]">
          <Filters view={view} community={community} />
          <Listings community={community} view={view} />
          <Standing view={view} />
        </div>
      )}
      <FilesDropOverlay />
    </div>
  );
}

function FilterButton({ on, onClick, children, label }: { on: boolean; onClick: () => void; children: React.ReactNode; label?: string }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      aria-label={label}
      onClick={onClick}
      className={cn(
        'flex h-8 w-full items-center gap-s-2 rounded-md px-s-2 text-left text-ui',
        on ? 'bg-selected text-text-strong' : 'text-muted-foreground hover:bg-hover hover:text-text',
      )}
    >
      {children}
    </button>
  );
}

const HEAD = 'px-s-2 pt-s-3 pb-s-1 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase';

// A category made from the Files page becomes the filter; one deleted there gives way to all files.
const newCategory = (community: CommunityView, view: FilesView) =>
  openCategoryEditor(community, view, null, (id) => useFilesUi.setState({ category: id ?? 'all' }));
const editCategory = (community: CommunityView, view: FilesView, c: CategoryView) =>
  openCategoryEditor(community, view, c, (id) => useFilesUi.setState({ category: id ?? 'all' }));

function Filters({ view, community }: { view: FilesView; community?: CommunityView }) {
  const ui = useFilesUi();
  const tags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const l of view.listings) for (const t of l.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 16).map(([t]) => t);
  }, [view.listings]);
  return (
    <nav aria-label="File filters" className="flex min-h-0 flex-col overflow-y-auto border-r border-border p-s-2">
      <div className="flex items-center justify-between pr-s-1">
        <h2 className={HEAD}>Categories</h2>
        {view.can_manage && community ? (
          <Tip label="New category">
            <button type="button" aria-label="New category" data-new-category className="grid size-6 place-items-center rounded text-muted-foreground hover:bg-hover hover:text-text" onClick={() => newCategory(community, view)}>
              <Plus className="size-4" aria-hidden />
            </button>
          </Tip>
        ) : null}
      </div>
      <FilterButton on={ui.category === 'all'} onClick={() => useFilesUi.setState({ category: 'all' })}>
        <span className="flex-1 truncate">All files</span>
        <span className="text-meta tabular-nums">{view.listings.length}</span>
      </FilterButton>
      {view.categories.map((c) => (
        <div key={c.id} className="group relative" onContextMenu={(e) => {
          if (!view.can_manage || !community || !c.readable) return;
          e.preventDefault();
          editCategory(community, view, c);
        }}>
          <FilterButton on={ui.category === c.id} onClick={() => useFilesUi.setState({ category: c.id })}>
            {c.readable ? null : <Lock className="size-3.5" aria-label="Locked" />}
            <span className="flex-1 truncate">{c.readable ? c.name : 'Locked category'}</span>
            {c.folder ? <Link2 className="size-3.5" aria-label="Linked to a folder" /> : null}
            <span className={cn('text-meta tabular-nums', view.can_manage && c.readable && 'group-hover:invisible group-focus-within:invisible')}>{c.files}</span>
          </FilterButton>
          {view.can_manage && community && c.readable ? (
            <button
              type="button"
              aria-label={`Edit ${c.name}`}
              data-edit-category={c.id}
              className="absolute top-1 right-1 grid size-6 place-items-center rounded text-muted-foreground opacity-0 group-hover:opacity-100 focus:opacity-100 hover:bg-hover hover:text-text"
              onClick={() => editCategory(community, view, c)}
            >
              <Pencil className="size-3.5" aria-hidden />
            </button>
          ) : null}
        </div>
      ))}
      <h2 className={HEAD}>Type</h2>
      {view.all_types.map((t) => {
        const on = !ui.hiddenTypes.includes(t);
        return (
          <label key={t} className="flex h-7 cursor-pointer items-center gap-s-2 px-s-2 text-ui text-text">
            <input
              type="checkbox"
              checked={on}
              onChange={() => useFilesUi.setState({ hiddenTypes: on ? [...ui.hiddenTypes, t] : ui.hiddenTypes.filter((x) => x !== t) })}
            />
            <span aria-hidden className="size-2.5 rounded-sm" style={tileStyle(t)} />
            {t}
          </label>
        );
      })}
      {tags.length ? <h2 className={HEAD}>Tags</h2> : null}
      <div className="flex flex-wrap gap-1 px-s-2">
        {tags.map((t) => (
          <button
            key={t}
            type="button"
            aria-pressed={ui.tag === t}
            onClick={() => useFilesUi.setState({ tag: ui.tag === t ? '' : t })}
            className={cn('rounded-full px-2 py-0.5 text-sm', ui.tag === t ? 'bg-accent text-white' : 'bg-elevated text-text hover:bg-hover')}
          >
            #{t}
          </button>
        ))}
      </div>
      <h2 className={HEAD}>Health</h2>
      {(
        [
          ['all', 'All', ''],
          ['well', 'Seeded well', '5+ seeders'],
          ['needs', 'Needs seeders', '1–4'],
          ['dead', 'No seeders', '0'],
        ] as const
      ).map(([key, label, hint]) => (
        <label key={key} className="flex h-7 cursor-pointer items-center gap-s-2 px-s-2 text-ui text-text">
          <input type="radio" name="files-health" checked={ui.health === key} onChange={() => useFilesUi.setState({ health: key })} />
          <span className="flex-1">{label}</span>
          <span className="text-meta text-muted-foreground">{hint}</span>
        </label>
      ))}
    </nav>
  );
}

const COLUMNS: { key: SortKey; label: string; right?: boolean }[] = [
  { key: 'name', label: 'Name' },
  { key: 'category', label: 'Category' },
  { key: 'size', label: 'Size', right: true },
  { key: 'seeders', label: 'Seeders', right: true },
  { key: 'leechers', label: 'Leechers', right: true },
  { key: 'completed', label: 'Completed', right: true },
  { key: 'added', label: 'Added', right: true },
  { key: 'uploader', label: 'Uploader', right: true },
];
const GRID = 'grid grid-cols-[minmax(0,3fr)_minmax(0,1.2fr)_80px_64px_64px_80px_64px_minmax(0,1.1fr)] items-center gap-s-2';

function filtered(view: FilesView, ui: FilesUi): ListingView[] {
  const cats = new Map(view.categories.map((c) => [c.id, c.name]));
  const q = ui.query.trim().toLowerCase();
  let list = view.listings.filter(
    (l) =>
      (ui.tab !== 'seeding' || l.local === 'seeding' || l.local === 'paused') &&
      (ui.tab !== 'uploads' || l.mine) &&
      (ui.category === 'all' || l.category === ui.category) &&
      !ui.hiddenTypes.includes(l.kind) &&
      (!ui.tag || l.tags.includes(ui.tag)) &&
      (ui.health === 'all' || healthOf(l) === ui.health) &&
      (!q || l.name.toLowerCase().includes(q) || l.tags.some((t) => t.includes(q.replace(/^#/, '')))),
  );
  const key = (l: ListingView): string | number => {
    switch (ui.sort) {
      case 'name': return l.name.toLowerCase();
      case 'category': return cats.get(l.category) ?? '';
      case 'size': return l.size;
      case 'seeders': return l.seeders;
      case 'leechers': return l.leechers;
      case 'completed': return l.completed;
      case 'added': return l.added_ms;
      case 'uploader': return l.lister_name.toLowerCase();
    }
  };
  list = [...list].sort((a, b) => {
    const x = key(a);
    const y = key(b);
    const c = x < y ? -1 : x > y ? 1 : 0;
    return ui.desc ? -c : c;
  });
  return list;
}

// Which page numbers to offer: the first, the last, and those around this one. Null stands for a gap.
export function pageNumbers(page: number, pages: number): (number | null)[] {
  const out: (number | null)[] = [];
  for (let n = 1; n <= pages; n++) {
    if (n === 1 || n === pages || Math.abs(n - page) <= 1) out.push(n);
    else if (out[out.length - 1] !== null) out.push(null);
  }
  return out;
}

function Pager({ page, pages, total, first, last, size }: { page: number; pages: number; total: number; first: number; last: number; size: number }) {
  const go = (n: number) => useFilesUi.setState({ page: n });
  return (
    <nav aria-label="File pages" data-files-pager className="flex shrink-0 flex-wrap items-center gap-s-2 border-t border-border px-s-4 py-s-2 text-sm text-muted-foreground">
      <span data-files-showing className="mr-auto tabular-nums">{total ? `Showing ${first}–${last} of ${total.toLocaleString()}` : 'Nothing to show'}</span>
      <label className="flex items-center gap-s-1">
        Per page
        <select aria-label="Files per page" value={size} onChange={(e) => setPageSize(Number(e.target.value))} className="h-8 rounded-md border border-input-border bg-input px-s-1 text-ui text-text">
          {PAGE_SIZES.map((n) => (
            <option key={n} value={n}>{n}</option>
          ))}
        </select>
      </label>
      <Button variant="ghost" size="sm" disabled={page <= 1} onClick={() => go(page - 1)}>Previous</Button>
      {pageNumbers(page, pages).map((n, i) =>
        n === null ? (
          <span key={`gap-${i}`} aria-hidden="true">…</span>
        ) : (
          <Button key={n} variant={n === page ? 'secondary' : 'ghost'} size="sm" aria-label={`Page ${n}`} aria-current={n === page ? 'page' : undefined} onClick={() => go(n)}>{n}</Button>
        ),
      )}
      <Button variant="ghost" size="sm" disabled={page >= pages} onClick={() => go(page + 1)}>Next</Button>
    </nav>
  );
}

function Listings({ community, view }: { community: CommunityView; view: FilesView }) {
  const ui = useFilesUi();
  // Only the filters and the catalog can change the result, so a page turn neither filters nor sorts again.
  const { tab, category, hiddenTypes, tag, health, sort, desc, query } = ui;
  const rows = useMemo(() => filtered(view, ui), [view, tab, category, hiddenTypes, tag, health, sort, desc, query]); // eslint-disable-line react-hooks/exhaustive-deps
  const pages = Math.max(1, Math.ceil(rows.length / ui.pageSize));
  // A list that shrank past this page moves to its last page, and stays there if it grows again.
  const page = Math.min(ui.page, pages);
  useEffect(() => {
    if (ui.page !== page) useFilesUi.setState({ page });
  }, [ui.page, page]);
  useEffect(() => {
    useFilesUi.setState({ page: 1 });
  }, [community.id]);
  const firstIndex = (page - 1) * ui.pageSize;
  const shown = rows.slice(firstIndex, firstIndex + ui.pageSize);
  const table = useRef<HTMLDivElement>(null);
  useEffect(() => {
    table.current?.scrollTo?.({ top: 0 });
  }, [page]);
  const cats = new Map(view.categories.map((c) => [c.id, c]));
  const target = ui.category !== 'all' ? cats.get(ui.category) : view.categories.find((c) => c.can_add && c.readable);
  const canAdd = !!target?.can_add && target.readable;
  const filterLine = [ui.category === 'all' ? 'All categories' : (cats.get(ui.category)?.name ?? ''), ui.tag ? `#${ui.tag}` : '', `sorted by ${COLUMNS.find((c) => c.key === ui.sort)?.label.toLowerCase()}`]
    .filter(Boolean)
    .join(' · ');
  return (
    <section aria-label="Files" className="flex min-h-0 flex-col">
      <div className="flex items-center gap-s-3 px-s-4 py-s-3">
        <div className="relative w-[280px]">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden />
          <Input aria-label="Search files" placeholder="Search files" className="pl-8" value={ui.query} onChange={(e) => useFilesUi.setState({ query: e.target.value })} />
        </div>
        <p className="m-0 min-w-0 flex-1 truncate text-sm text-muted-foreground" data-files-count>
          {rows.length} {rows.length === 1 ? 'file' : 'files'} · {filterLine}
        </p>
        <Tip label={canAdd ? `Add files to ${target?.name}` : view.categories.length ? 'You cannot add to this category' : 'No categories yet'}>
          <Button disabled={!canAdd} onClick={() => target && openAddFiles(community, view, target)}>
            <Plus className="size-4" /> Add files
          </Button>
        </Tip>
        <Tip label={view.categories.some((c) => c.can_add && c.readable) ? 'Add a whole folder, seeded where it is' : 'No category you can add to'}>
          <Button variant="secondary" disabled={!view.categories.some((c) => c.can_add && c.readable)} onClick={() => void openAddFolder(community, view)}>
            <FolderPlus className="size-4" /> Add folder
          </Button>
        </Tip>
        {canAdd && target?.folder ? (
          <Tip label={`Look for new files in ${target.folder}`}>
            <Button variant="ghost" onClick={() => void openAddFiles(community, view, target, true)}>
              <FolderOpen className="size-4" /> Scan folder
            </Button>
          </Tip>
        ) : null}
      </div>
      <div ref={table} role="table" aria-label="File list" className="flex min-h-0 flex-1 flex-col overflow-y-auto px-s-4 pb-s-4">
        <div role="row" className={cn(GRID, 'sticky top-0 z-[1] h-9 bg-chat text-meta font-semibold text-muted-foreground uppercase')}>
          {COLUMNS.map((c) => (
            <button
              key={c.key}
              type="button"
              role="columnheader"
              aria-sort={ui.sort === c.key ? (ui.desc ? 'descending' : 'ascending') : 'none'}
              onClick={() => useFilesUi.setState({ sort: c.key, desc: ui.sort === c.key ? !ui.desc : true })}
              className={cn('truncate', c.right && 'text-right', ui.sort === c.key && 'text-text-strong')}
            >
              {c.label} {ui.sort === c.key ? (ui.desc ? '▼' : '▲') : ''}
            </button>
          ))}
        </div>
        {shown.map((l) => (
          <div
            key={l.id}
            role="row"
            tabIndex={0}
            data-listing={l.id}
            onClick={() => useFilesUi.setState({ open: l.id })}
            onKeyDown={(e) => e.key === 'Enter' && useFilesUi.setState({ open: l.id })}
            className={cn(GRID, 'h-12 cursor-pointer rounded-md border-t border-border text-ui hover:bg-hover')}
          >
            <span role="cell" className="flex min-w-0 items-center gap-s-2">
              <span className="grid h-8 w-10 shrink-0 place-items-center rounded text-[10px] font-bold text-white" style={tileStyle(l.kind)}>{l.ext || 'FILE'}</span>
              <span className="min-w-0">
                <span className="block truncate font-medium text-text-strong">{l.name}</span>
                <span className="block truncate text-meta text-muted-foreground">
                  {l.folder ? `${l.folder.files.toLocaleString()} ${l.folder.files === 1 ? 'file' : 'files'} · ` : ''}
                  {l.tags.slice(0, 3).map((t) => `#${t}`).join(' ')}
                </span>
              </span>
              {l.folder ? (
                <button
                  type="button"
                  data-folder-contents-button
                  className="ml-auto shrink-0 rounded-md px-s-2 py-0.5 text-sm text-link hover:bg-hover"
                  onClick={(e) => {
                    e.stopPropagation();
                    openFolderContents(community.id, l);
                  }}
                >
                  Contents
                </button>
              ) : null}
            </span>
            <span role="cell" className="truncate text-muted-foreground">{cats.get(l.category)?.name}</span>
            <span role="cell" className="text-right tabular-nums">{formatBytes(l.size)}</span>
            <span role="cell" className={cn('text-right font-semibold tabular-nums', SEED_COLOR[healthOf(l)])}>{l.seeders}</span>
            <span role="cell" className="text-right tabular-nums">{l.leechers}</span>
            <span role="cell" className="text-right tabular-nums">{l.completed.toLocaleString()}</span>
            <span role="cell" className="text-right text-muted-foreground">{dateOf(l.added_ms)}</span>
            <span role="cell" className="truncate text-right">{l.lister_name}</span>
          </div>
        ))}
        {!rows.length ? <p className="py-s-8 text-center text-body text-muted-foreground">{view.listings.length ? 'No files match these filters.' : view.categories.length || !view.can_manage ? 'No files listed yet. Drop files here to add them.' : null}</p> : null}
        {!rows.length && !view.categories.length && view.can_manage ? (
          <div className="flex justify-center">
            <Button data-first-category onClick={() => newCategory(community, view)}>
              <Plus className="size-4" /> Create the first category
            </Button>
          </div>
        ) : null}
        {view.locked ? <p className="text-sm text-muted-foreground">{view.locked} more are locked until a member shares this server’s files key.</p> : null}
      </div>
      <Pager page={page} pages={pages} total={rows.length} first={firstIndex + 1} last={firstIndex + shown.length} size={ui.pageSize} />
    </section>
  );
}

function Standing({ view }: { view: FilesView }) {
  const s = view.standing;
  const stats: [string, string][] = [
    ['Ratio', s.ratio === null ? '—' : s.ratio.toFixed(2)],
    ['Uploaded', formatBytes(s.uploaded)],
    ['Downloaded', formatBytes(s.downloaded)],
    ['Served', `${s.served_members} members`],
  ];
  return (
    <aside aria-label="Your standing" data-standing className="flex min-h-0 flex-col gap-s-3 overflow-y-auto border-l border-border p-s-4">
      <h2 className="m-0 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">Your standing</h2>
      <p className="m-0 text-head font-bold text-text-strong">{formatLevel(s)}</p>
      {s.next ? (
        <div className="flex flex-col gap-1">
          <div className="flex justify-between text-sm text-muted-foreground">
            <span>To Level {s.next.level} · {s.next.name}</span>
            <span>{Math.round(s.next.progress * 100)}%</span>
          </div>
          <div className="h-1.5 rounded-full bg-elevated">
            <div className="h-full rounded-full bg-accent" style={{ width: `${Math.round(s.next.progress * 100)}%` }} />
          </div>
          <p className="m-0 text-sm text-muted-foreground">
            {s.next.bytes_needed > 0 ? `Seed ${formatBytes(s.next.bytes_needed)} more` : 'Enough bytes seeded'}
            {s.next.members_needed > 0 ? ` to ${s.next.members_needed} more members you haven’t served before.` : '.'}
          </p>
        </div>
      ) : null}
      <dl className="m-0 grid grid-cols-2 gap-s-2">
        {stats.map(([k, v]) => (
          <div key={k} className="rounded-md bg-panel p-s-2">
            <dt className="text-meta text-muted-foreground">{k}</dt>
            <dd className="m-0 text-ui font-semibold text-text-strong">{v}</dd>
          </div>
        ))}
      </dl>
      <p className="m-0 text-sm text-muted-foreground">
        Reputation comes from signed receipts that members send when they finish downloading from you. Each member counts once
        {view.rules.decay_days ? `, and credit fades after ${view.rules.decay_days} days without seeding` : ''}.
      </p>
    </aside>
  );
}

function FilePage({ community, view, listing: l }: { community: CommunityView; view: FilesView; listing: ListingView }) {
  const category = view.categories.find((c) => c.id === l.category);
  const [busy, setBusy] = useState(false);
  const own = useShell((s) => s.transfers.find((t) => t.message_id === `files:${community.id}:${l.id}`));
  const download = async () => {
    setBusy(true);
    try {
      await downloadWithin((force) => (l.folder ? filesApi.downloadFolder(community.id, l.id, force) : filesApi.download(community.id, l.id, force)));
      await useShell.getState().loadTransfers();
      await reloadFiles(community.id);
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };
  const remove = async () => {
    if (!(await confirm({ title: `Remove ${l.name}?`, message: 'The listing goes away for everyone. Members who have the file keep it.', okLabel: 'Remove', danger: true }))) return;
    try {
      await filesApi.removeListing(community.id, l.id);
      useFilesUi.setState({ open: null });
      await reloadFiles(community.id);
    } catch (err) {
      toast(err);
    }
  };
  const facts: [string, string, boolean?][] = [
    [l.folder ? 'Contents fingerprint' : 'Fingerprint', `b3:${l.fingerprint.slice(0, 6)}…${l.fingerprint.slice(-6)}`, true],
    ['Size', `${formatBytes(l.size)} · ${l.size.toLocaleString()} bytes`],
    l.folder ? ['Files', l.folder.files.toLocaleString()] : ['Pieces', `${l.pieces.toLocaleString()} × 64 KiB`],
    ['Category', category?.name ?? ''],
    ['Encryption', 'AES-256-GCM, server key'],
  ];
  if (category?.folder) facts.splice(4, 0, ['Linked folder', category.folder, true]);
  return (
    <div data-file-page className="flex min-h-0 flex-col gap-s-4 overflow-y-auto p-s-6">
      <nav aria-label="Breadcrumb" className="flex items-center gap-s-2 text-sm text-muted-foreground">
        <button type="button" className="inline-flex items-center gap-1 hover:text-text" onClick={() => useFilesUi.setState({ open: null })}>
          <ArrowLeft className="size-4" /> Files
        </button>
        <span>›</span>
        <span>{category?.name}</span>
      </nav>
      <header className="flex items-start gap-s-4">
        <span className="grid size-14 shrink-0 place-items-center rounded-lg text-sm font-bold text-white" style={tileStyle(l.kind)}>{l.ext || 'FILE'}</span>
        <div className="min-w-0 flex-1">
          <h2 className="m-0 truncate text-title font-bold text-text-strong">{l.name}</h2>
          <p className="m-0 text-sm text-muted-foreground">
            {l.kind} · {formatBytes(l.size)} · Added {dateOf(l.added_ms)} by {l.lister_name}
          </p>
          <div className="mt-1 flex flex-wrap gap-1">
            {l.tags.map((t) => (
              <span key={t} className="rounded-full bg-elevated px-2 py-0.5 text-sm text-text">#{t}</span>
            ))}
          </div>
        </div>
        <div className="flex shrink-0 flex-col items-end gap-s-1">
          {l.local === 'downloaded' || l.local === 'seeding' || l.local === 'paused' ? (
            <span className="text-sm font-semibold text-success-text">{l.local === 'downloaded' ? 'Downloaded' : l.local === 'paused' ? 'Seeding paused' : l.folder ? 'You seed this folder' : 'You seed this file'}</span>
          ) : l.local === 'downloading' ? (
            <span data-download-progress className="text-sm font-semibold text-text tabular-nums">Downloading · {Math.round((own?.progress ?? l.progress) * 100)}%{own?.rate_bps ? ` · ${formatBytes(own.rate_bps)}/s` : ''}</span>
          ) : (
            <Button disabled={busy} onClick={() => void download()}>
              <Download className="size-4" /> Download · {formatBytes(l.size)}
            </Button>
          )}
          <span className="text-meta text-muted-foreground">From {l.seeders} {l.seeders === 1 ? 'seeder' : 'seeders'}</span>
          {l.over_limit ? <span data-over-limit className="text-meta text-warn">{l.over_limit}</span> : null}
          {l.size > RELEASED_LIMIT ? <span data-older-note className="max-w-[260px] text-right text-meta text-muted-foreground">{OLDER_VERSIONS_NOTE}</span> : null}
          {l.can_edit ? (
            <Button variant="ghost" onClick={() => openEditListing(community.id, l)}>
              <Pencil className="size-4" /> Edit tags and description
            </Button>
          ) : null}
          {l.can_edit ? (
            <Button variant="ghost" onClick={() => void remove()}>
              <Trash2 className="size-4" /> Remove listing
            </Button>
          ) : null}
        </div>
      </header>
      {l.danger ? <DangerNote danger={l.danger} /> : null}
      {l.description ? <p className="m-0 max-w-[760px] text-body text-text whitespace-pre-wrap">{l.description}</p> : null}
      <dl data-file-facts className="m-0 grid max-w-[760px] grid-cols-[160px_1fr] gap-x-s-4 gap-y-s-2">
        {facts.map(([k, v, mono]) => (
          <div key={k} className="contents">
            <dt className="text-sm text-muted-foreground">{k}</dt>
            <dd className={cn('m-0 text-ui text-text-strong', mono && 'font-mono text-sm')} title={k === 'Fingerprint' ? `b3:${l.fingerprint}` : undefined}>{v}</dd>
          </div>
        ))}
      </dl>
      {l.folder ? (
        <>
          <FolderDownload communityId={community.id} listing={l} />
          <div className="max-w-[760px]">
            <FolderContents communityId={community.id} listing={l} />
          </div>
        </>
      ) : (
        <SwarmPanel communityId={community.id} listing={l} />
      )}
      <section aria-label="Activity" className="flex max-w-[760px] flex-col gap-1 text-sm text-text">
        <p className="m-0">Downloaded completely {l.completed.toLocaleString()} {l.completed === 1 ? 'time' : 'times'}</p>
        <p className="m-0">First listed by {l.lister_name} · {dateOf(l.added_ms)}</p>
        {l.also_listed_by.length ? <p className="m-0 text-muted-foreground">Also listed by {l.also_listed_by.join(', ')} (same file, merged)</p> : null}
      </section>
      {l.local_path ? (
        <p className="m-0 flex items-center gap-s-2 text-sm text-muted-foreground">
          <FolderOpen className="size-4" /> {l.local_path}
        </p>
      ) : null}
      <section aria-label="Integrity" className="max-w-[760px] rounded-lg border border-border bg-panel p-s-4 text-sm text-text">
        <p className="m-0">Every piece is checked against the file’s fingerprint as it arrives, so a bad seeder can’t slip in wrong bytes.</p>
        <p className="m-0 mt-1">Nothing is opened until the whole file is in and verified.</p>
      </section>
    </div>
  );
}

export function categoryLabel(c: CategoryView): string {
  return c.readable ? c.name : 'Locked category';
}
