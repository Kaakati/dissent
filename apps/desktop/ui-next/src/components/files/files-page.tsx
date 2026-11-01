// The server's Files area (the tracker): Browse with category, type, tag and health filters and a sortable table, the
// file page, My seeding and Uploads. Opened from the server sidebar. See docs/design/14-file-tracker.md.
import { useMemo, useState } from 'react';
import { create } from 'zustand';
import { ArrowLeft, Download, FolderOpen, Link2, Lock, Plus, Search, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Tip } from '@/components/ui/tip';
import { formatBytes } from '@/components/transfers/format';
import { confirm } from '@/lib/dialog';
import { filesApi, formatLevel, reloadFiles, tileStyle, useFilesOf, type CategoryView, type FilesView, type ListingView } from '@/lib/files';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { DangerNote } from '@/components/chat/attachments';
import '@/components/chat/chat.css';
import { openAddFiles } from './add-files-dialog';
import { SeedingPanel } from './seeding-panel';
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
}));

const healthOf = (l: ListingView): Health => (l.seeders >= 5 ? 'well' : l.seeders > 0 ? 'needs' : 'dead');
const SEED_COLOR: Record<Health, string> = { all: '', well: 'text-success-text', needs: 'text-warn', dead: 'text-danger-text' };
const dateOf = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });

function useCommunityFiles(): { community: CommunityView | null; view: FilesView | null } {
  const community = useShell((s) => (s.route.kind === 'server' ? s.community : null));
  const transfers = useShell((s) => s.transfers);
  const view = useFilesOf(community?.id ?? null, community, transfers);
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
  if (open) return <FilePage community={community} view={view} listing={open} />;
  if (ui.tab === 'seeding') return <SeedingPanel communityId={community.id} trigger={view} />;
  return (
    <div data-files className="grid min-h-0 grid-cols-[220px_minmax(0,1fr)_260px]">
      <Filters view={view} />
      <Listings community={community} view={view} />
      <Standing view={view} />
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

function Filters({ view }: { view: FilesView }) {
  const ui = useFilesUi();
  const tags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const l of view.listings) for (const t of l.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 16).map(([t]) => t);
  }, [view.listings]);
  return (
    <nav aria-label="File filters" className="flex min-h-0 flex-col overflow-y-auto border-r border-border p-s-2">
      <h2 className={HEAD}>Categories</h2>
      <FilterButton on={ui.category === 'all'} onClick={() => useFilesUi.setState({ category: 'all' })}>
        <span className="flex-1 truncate">All files</span>
        <span className="text-meta tabular-nums">{view.listings.length}</span>
      </FilterButton>
      {view.categories.map((c) => (
        <FilterButton key={c.id} on={ui.category === c.id} onClick={() => useFilesUi.setState({ category: c.id })}>
          {c.readable ? null : <Lock className="size-3.5" aria-label="Locked" />}
          <span className="flex-1 truncate">{c.readable ? c.name : 'Locked category'}</span>
          {c.folder ? <Link2 className="size-3.5" aria-label="Linked to a folder" /> : null}
          <span className="text-meta tabular-nums">{c.files}</span>
        </FilterButton>
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

function Listings({ community, view }: { community: CommunityView; view: FilesView }) {
  const ui = useFilesUi();
  const rows = filtered(view, ui);
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
        {canAdd && target?.folder ? (
          <Tip label={`Look for new files in ${target.folder}`}>
            <Button variant="ghost" onClick={() => void openAddFiles(community, view, target, true)}>
              <FolderOpen className="size-4" /> Scan folder
            </Button>
          </Tip>
        ) : null}
      </div>
      <div role="table" aria-label="File list" className="flex min-h-0 flex-col overflow-y-auto px-s-4 pb-s-4">
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
        {rows.map((l) => (
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
                <span className="block truncate text-meta text-muted-foreground">{l.tags.slice(0, 3).map((t) => `#${t}`).join(' ')}</span>
              </span>
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
        {!rows.length ? <p className="py-s-8 text-center text-body text-muted-foreground">{view.listings.length ? 'No files match these filters.' : 'No files listed yet.'}</p> : null}
        {view.locked ? <p className="text-sm text-muted-foreground">{view.locked} more are locked until a member shares this server’s files key.</p> : null}
      </div>
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
  const download = async () => {
    setBusy(true);
    try {
      await filesApi.download(community.id, l.id);
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
    ['Fingerprint', `b3:${l.fingerprint.slice(0, 6)}…${l.fingerprint.slice(-6)}`, true],
    ['Size', `${formatBytes(l.size)} · ${l.size.toLocaleString()} bytes`],
    ['Pieces', `${l.pieces.toLocaleString()} × 64 KiB`],
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
            <span className="text-sm font-semibold text-success-text">{l.local === 'downloaded' ? 'Downloaded' : l.local === 'paused' ? 'Seeding paused' : 'You seed this file'}</span>
          ) : l.local === 'downloading' ? (
            <span className="text-sm font-semibold text-text">Downloading · {Math.round(l.progress * 100)}%</span>
          ) : (
            <Button disabled={busy} onClick={() => void download()}>
              <Download className="size-4" /> Download · {formatBytes(l.size)}
            </Button>
          )}
          <span className="text-meta text-muted-foreground">From {l.seeders} {l.seeders === 1 ? 'seeder' : 'seeders'}</span>
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
      <SwarmPanel communityId={community.id} listing={l} />
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
