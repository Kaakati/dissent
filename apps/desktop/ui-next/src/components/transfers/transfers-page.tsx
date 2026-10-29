// The Transfers page (views/pages.js): Active, Completed and Failed tabs in the header, the rows in the main column, and
// the storage panel in the right column. The actions on each row go through lib/transfers.ts. Ported from transfersPage().
import { useEffect, type ReactNode } from 'react';
import { create } from 'zustand';
import { Folder, Pause, Play, RotateCcw, Trash2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { useShell } from '@/lib/store';
import {
  cancelTransfer,
  changeDownloadDir,
  pauseTransfer,
  removeTransfer,
  resetDownloadDir,
  resumeTransfer,
  retryTransfer,
  revealTransfer,
  setAutoAccept,
  useTransferSettings,
} from '@/lib/transfers';
import type { TransferItem, TransferRouteKind } from '@/lib/types';
import { cn } from '@/lib/utils';
import { formatBytes } from './format';
import { ProgressBar } from './progress';
import { StoreMeter } from './store-meter';

type TransferTab = 'active' | 'done' | 'failed';

const TABS: { key: TransferTab; label: string; empty: string }[] = [
  { key: 'active', label: 'Active', empty: 'No active transfers.' },
  { key: 'done', label: 'Completed', empty: 'No completed transfers.' },
  { key: 'failed', label: 'Failed', empty: 'No failed transfers.' },
];

// The route tag colour for each route kind. Anything else is muted.
const ROUTE_TAG: Partial<Record<TransferRouteKind, string>> = {
  direct: 'text-success-text',
  server: 'text-success-text',
  seeder: 'text-success-text',
  relay: 'text-warn',
  waiting: 'text-muted-foreground',
  refused: 'text-danger-text',
  failed: 'text-danger-text',
};

// The tab stays open across visits to the page, as the current UI keeps it.
const useTransferTab = create<{ tab: TransferTab; setTab: (tab: TransferTab) => void }>()((set) => ({
  tab: 'active',
  setTab: (tab) => set({ tab }),
}));

const COLUMNS = 'grid-cols-[minmax(0,2.2fr)_minmax(0,1.4fr)_110px_minmax(0,1.5fr)_minmax(0,1.6fr)_96px]';

const extOf = (name: string): string => (name.includes('.') ? (name.split('.').pop() ?? '').toUpperCase() : 'FILE');

function groupOf(rows: TransferItem[]): Record<TransferTab, TransferItem[]> {
  return {
    active: rows.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)),
    done: rows.filter((t) => t.state === 'done'),
    failed: rows.filter((t) => t.state === 'failed'),
  };
}

// The tabs sit in the page header, straight after the title.
export function TransfersTabs() {
  const transfers = useShell((s) => s.transfers);
  const tab = useTransferTab((s) => s.tab);
  const setTab = useTransferTab((s) => s.setTab);
  const groups = groupOf(transfers);
  return (
    <div role="tablist" aria-label="Transfer state" className="flex gap-s-2">
      {TABS.map((t) => {
        const on = t.key === tab;
        return (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={on}
            onClick={() => setTab(t.key)}
            className={cn(
              'inline-flex h-8 items-center gap-s-2 rounded-md px-s-3 text-ui font-semibold transition-colors',
              on ? 'bg-selected text-text-strong' : 'text-muted-foreground hover:bg-hover hover:text-text',
            )}
          >
            <span>{t.label}</span>
            <span className="min-w-5 rounded-full bg-elevated px-1.5 text-center text-meta leading-[18px] font-semibold text-text tabular-nums">
              {groups[t.key].length}
            </span>
          </button>
        );
      })}
    </div>
  );
}

// The list of the chosen tab, with its header row. Scrolls on its own.
export function TransfersBody() {
  const transfers = useShell((s) => s.transfers);
  const tab = useTransferTab((s) => s.tab);
  const current = TABS.find((t) => t.key === tab) ?? TABS[0];
  const list = groupOf(transfers)[current.key];
  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-s-3 overflow-y-auto px-s-6 py-s-4">
      <div role="table" aria-label="Transfers" className="flex flex-col">
        <div role="row" className={cn('grid h-10 items-center gap-s-4 px-s-3 text-meta font-semibold tracking-[0.02em] text-muted-foreground uppercase', COLUMNS)}>
          <span role="columnheader">File</span>
          <span role="columnheader">Shared in</span>
          <span role="columnheader">Direction</span>
          <span role="columnheader">Route</span>
          <span role="columnheader">Progress</span>
          <span role="columnheader">Actions</span>
        </div>
        {list.length ? (
          list.map((t) => <TransferRow key={t.id} transfer={t} />)
        ) : (
          <p className="mx-auto mt-16 flex max-w-90 flex-col items-center gap-1.5 text-center text-sm text-muted-foreground">{current.empty}</p>
        )}
      </div>
    </div>
  );
}

function TransferRow({ transfer: t }: { transfer: TransferItem }) {
  const where = t.where ?? { kind: 'server' as const, name: '' };
  const pct = Math.round((Number(t.progress) || 0) * 100);
  const showBar = t.state === 'active' || t.state === 'paused';
  return (
    <div role="row" className={cn('grid min-h-18 items-center gap-s-4 rounded-lg px-s-3 py-2.5 shadow-[inset_0_1px_0_var(--border)]', COLUMNS)}>
      <div role="cell" className="flex min-w-0 items-center gap-s-2">
        <span className="grid h-11 w-9 flex-none place-items-center rounded-md text-[9px] font-bold tracking-[0.04em] text-white" style={{ background: 'oklch(45% 0.14 220)' }} aria-hidden="true">
          {extOf(t.name)}
        </span>
        <span className="flex min-w-0 flex-col">
          <span className="truncate text-ui font-semibold text-text-strong">{t.name}</span>
          <span className="text-[12px] leading-[18px] tabular-nums text-muted-foreground">{formatBytes(t.size)}</span>
        </span>
      </div>
      <div role="cell" className="flex min-w-0 items-center gap-s-2">
        <WhereAvatar name={where.name} square={where.kind !== 'direct'} />
        <span className="flex min-w-0 flex-col">
          <span className="truncate text-sm font-medium text-text">{where.name}</span>
          <span className="text-meta text-muted-foreground">{where.kind === 'direct' ? 'Direct message' : 'Server channel'}</span>
        </span>
      </div>
      <div role="cell" className="text-sm text-text">{t.direction === 'upload' ? '↑ Upload' : '↓ Download'}</div>
      <div role="cell" className="flex min-w-0 flex-col items-start gap-1">
        <span className={cn('inline-flex h-[22px] items-center rounded-sm bg-white/6 px-s-2 text-meta leading-[21px] font-semibold whitespace-nowrap', ROUTE_TAG[t.route_kind] ?? 'text-muted-foreground')}>
          {t.route}
        </span>
        <span className="text-meta text-muted-foreground">{t.route_note ?? ''}</span>
      </div>
      <div role="cell" className="flex min-w-0 flex-col items-start gap-1">
        {showBar ? <ProgressBar value={pct / 100} thin className="w-full" /> : null}
        <span className="text-[12px] leading-[18px] tabular-nums">{t.status_text ?? ''}</span>
      </div>
      <div role="cell" className="flex justify-end gap-1.5">
        <TransferActions transfer={t} />
      </div>
    </div>
  );
}

// The round buttons for a row's state: active pauses, paused resumes, failed retries, done shows the file's folder.
function TransferActions({ transfer: t }: { transfer: TransferItem }) {
  const round = (label: string, icon: ReactNode, onClick: () => void) => (
    <Tip label={label}>
      <Button variant="ghost" size="icon" aria-label={label} onClick={onClick} className="size-9 rounded-full bg-sidebar text-text hover:bg-sidebar hover:text-text">
        {icon}
      </Button>
    </Tip>
  );
  const cancel = round('Cancel', <X className="size-4" />, () => void cancelTransfer(t.id));
  const remove = round('Remove from list', <Trash2 className="size-4" />, () => void removeTransfer(t.id));
  switch (t.state) {
    case 'active':
      return <>{round('Pause', <Pause className="size-4" />, () => void pauseTransfer(t.id))}{cancel}</>;
    case 'paused':
      return <>{round('Resume', <Play className="size-4" />, () => void resumeTransfer(t.id))}{cancel}</>;
    case 'failed':
      return <>{round('Retry', <RotateCcw className="size-4" />, () => void retryTransfer(t.id))}{remove}</>;
    case 'done':
      return <>{round('Show in folder', <Folder className="size-4" />, () => t.local_path && void revealTransfer(t.local_path))}{remove}</>;
    default:
      return cancel;
  }
}

// The mark of the conversation or server a transfer is shared in: its first two letters, on the default hue as in the
// current UI. Square for a server, round for a direct message.
function WhereAvatar({ name, square }: { name: string; square: boolean }) {
  return (
    <span
      className={cn('grid size-6 flex-none place-items-center text-[8px] font-semibold text-white', square ? 'rounded-lg' : 'rounded-full')}
      style={{ background: 'oklch(48% 0.14 220)' }}
      aria-hidden="true"
    >
      {(name || '?').slice(0, 2).toUpperCase()}
    </span>
  );
}

// The Storage panel in the right column: the store meter, the largest file accepted, auto-accept and the download folder.
export function TransfersAside() {
  const settings = useTransferSettings((s) => s.settings);
  const load = useTransferSettings((s) => s.load);
  const transfers = useShell((s) => s.transfers);
  const max = settings?.max_accept_bytes ?? 10 * 1024 ** 3;
  // The settings are read again whenever the list changes, as the current UI does on every transfer event.
  useEffect(() => {
    void load();
  }, [load, transfers]);
  return (
    <aside aria-label="Storage" className="flex min-h-0 flex-col gap-s-4 overflow-y-auto p-s-4">
      <h3 className="text-[17px] leading-6 font-bold text-text-strong">Storage</h3>
      <p className="text-sm text-muted-foreground">Partial and cancelled files are cleared within the hour.</p>
      <StoreMeter />
      <div className="rounded-lg bg-sidebar p-s-4 text-center">
        <div className="text-ui font-semibold text-text-strong">Largest file you’ll accept</div>
        <div className="mt-1 text-[12px] leading-[18px] text-muted-foreground">{formatBytes(max)}. Bigger files are refused before any data moves.</div>
      </div>
      {settings && 'auto_accept_bytes' in settings ? <AutoAccept value={settings.auto_accept_bytes ?? null} /> : null}
      <div className="flex flex-col gap-s-2">
        <h4 className="text-sm font-semibold text-text-strong">Save downloads to</h4>
        <div className="text-meta break-all text-muted-foreground">{settings?.download_dir ?? 'Not set'}</div>
        <Button variant="outline" className="self-start" disabled={!settings} onClick={() => void changeDownloadDir()}>
          Change
        </Button>
        {settings && !settings.download_dir_is_default ? (
          <Button variant="ghost" className="self-start" onClick={() => void resetDownloadDir()}>
            Reset to default
          </Button>
        ) : null}
      </div>
    </aside>
  );
}

const AUTO_ACCEPT_OPTIONS: { bytes: number | null; label: string }[] = [
  { bytes: 25 * 1024 ** 2, label: '25 MB' },
  { bytes: 50 * 1024 ** 2, label: '50 MB' },
  { bytes: 100 * 1024 ** 2, label: '100 MB' },
  { bytes: null, label: 'Always ask' },
];

function AutoAccept({ value }: { value: number | null }) {
  return (
    <div className="flex flex-col gap-s-2">
      <h4 className="text-sm font-semibold text-text-strong">Auto-accept files up to</h4>
      <div role="group" aria-label="Auto-accept files up to" className="flex flex-wrap gap-s-2">
        {AUTO_ACCEPT_OPTIONS.map((opt) => {
          const pressed = value === opt.bytes;
          return (
            <Button
              key={opt.label}
              variant="outline"
              aria-pressed={pressed}
              onClick={() => void setAutoAccept(opt.bytes)}
              className={cn('h-8 rounded-md px-s-3', pressed && 'bg-selected text-text-strong shadow-[inset_0_0_0_1px_var(--accent)]')}
            >
              {opt.label}
            </Button>
          );
        })}
      </div>
    </div>
  );
}
