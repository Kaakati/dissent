// The encrypted file store: used bytes against the budget, with a segment for each kind of file and a legend.
// Ported from storeMeter() in views/pages.js. It reads store_stats again whenever the transfer list changes.
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import type { StoreStats } from '@/lib/types';
import { formatBytes } from './format';

// The segment colours are the current UI's, not tokens: accent for shared files, blue for downloads, grey for partials.
const PARTS = [
  { key: 'shared_bytes', label: 'Files you shared', color: '#d2452f' },
  { key: 'downloaded_bytes', label: 'Files you downloaded', color: '#4f8cff' },
  { key: 'partial_bytes', label: 'Partial and cancelled', color: '#6b6f78' },
] as const;

export function StoreMeter() {
  const transfers = useShell((s) => s.transfers);
  const [stats, setStats] = useState<StoreStats | null>(null);

  useEffect(() => {
    let live = true;
    api.storeStats().then((s) => {
      if (live) setStats(s);
    }).catch(() => {});
    return () => {
      live = false;
    };
  }, [transfers]);

  if (!stats) return null;
  const budget = stats.budget_bytes;
  const pct = budget ? Math.min(100, (stats.used_bytes / budget) * 100) : 0;
  const parts = PARTS.map((p) => ({ ...p, bytes: stats[p.key] })).filter((p): p is typeof p & { bytes: number } => typeof p.bytes === 'number');

  return (
    <div className="flex flex-col gap-s-2">
      <div className="flex items-center justify-between text-ui text-text">
        <span>Encrypted file store</span>
        <span className="tabular-nums">{formatBytes(stats.used_bytes)}</span>
      </div>
      <div
        role="meter"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
        aria-label="Encrypted file store"
        className="flex h-1.5 overflow-hidden rounded-[3px] bg-elevated"
      >
        {parts.length ? (
          parts.map((p) => (
            <span key={p.key} className="block h-full" style={{ width: `${budget ? (p.bytes / budget) * 100 : 0}%`, background: p.color }} />
          ))
        ) : (
          <span className="block h-full w-full origin-left bg-accent transition-transform duration-200 ease-out" style={{ transform: `scaleX(${pct / 100})` }} />
        )}
      </div>
      {parts.length ? (
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {parts.map((p) => (
            <li key={p.key} className="grid grid-cols-[10px_minmax(0,1fr)_auto] items-center gap-s-2 text-meta text-muted-foreground">
              <span className="size-2 rounded-[2px]" style={{ background: p.color }} aria-hidden="true" />
              <span>{p.label}</span>
              <span className="tabular-nums">{formatBytes(p.bytes)}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
