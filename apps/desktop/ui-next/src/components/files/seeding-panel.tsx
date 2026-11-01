// My seeding: what this computer seeds in the open server, each file's state and limit, and the speed and limit
// settings, which hold for every server. See dissent_core::tracker::seeding.
import { useEffect, useState } from 'react';
import { Minus, Pause, Play, Plus, Square } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Tip } from '@/components/ui/tip';
import { formatBytes } from '@/components/transfers/format';
import { limitLabel, seedingApi, tileStyle, type SeedingSettings, type SeedingView, type SeedRowView, type SeedState } from '@/lib/files';
import { toast } from '@/lib/toasts';
import type { Id } from '@/lib/types';
import { cn } from '@/lib/utils';

const MB = 1024 * 1024;
const STATE_LABEL: Record<SeedState, string> = { seeding: 'Seeding', paused: 'Paused', scheduled: 'Scheduled', queued: 'Queued', stopped: 'Stopped' };
const STATE_CLASS: Record<SeedState, string> = {
  seeding: 'bg-[rgba(35,165,90,.15)] text-success-text',
  paused: 'bg-[rgba(240,178,50,.15)] text-warn',
  scheduled: 'bg-[rgba(124,183,255,.15)] text-info',
  queued: 'bg-elevated text-muted-foreground',
  stopped: 'bg-[rgba(208,16,90,.15)] text-danger-text',
};
const DAYS = ['M', 'T', 'W', 'T', 'F', 'S', 'S'];
const hhmm = (min: number) => `${String(Math.floor(min / 60)).padStart(2, '0')}:${String(min % 60).padStart(2, '0')}`;
const minOf = (text: string) => {
  const [h, m] = text.split(':').map(Number);
  return Math.max(0, Math.min(24 * 60 - 1, (h || 0) * 60 + (m || 0)));
};

export function SeedingPanel({ communityId, trigger }: { communityId: Id; trigger: unknown }) {
  const [view, setView] = useState<SeedingView | null>(null);
  const [tick, setTick] = useState(0);
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    seedingApi
      .view(communityId)
      .then((v) => {
        if (live) setView(v);
      })
      .catch((err: unknown) => {
        if (live) toast(err);
      });
    return () => {
      live = false;
    };
  }, [communityId, trigger, tick]);
  // The schedule runs on local time, which only the page knows.
  useEffect(() => {
    void seedingApi.setUtcOffset(-new Date().getTimezoneOffset()).catch(() => undefined);
  }, []);
  if (!view) return <p className="p-s-6 text-body text-muted-foreground">Loading…</p>;
  const reload = () => setTick((n) => n + 1);
  const act = async (f: () => Promise<void>) => {
    try {
      await f();
      reload();
    } catch (err) {
      toast(err);
    }
  };
  const anyActive = view.rows.some((r) => r.state === 'seeding');
  return (
    <div data-seeding className="grid min-h-0 grid-cols-[minmax(0,1fr)_320px]">
      <section aria-label="My seeding" className="flex min-h-0 flex-col gap-s-3 overflow-y-auto p-s-4">
        <div className="grid grid-cols-3 gap-s-2">
          <Tile k="Seeding" v={String(view.seeding)} sub={`of ${view.rows.length} files · max ${view.settings.max_active} at once`} />
          <Tile k="Uploaded" v={formatBytes(view.uploaded)} sub="from these files" />
          <Tile k="Right now" v={view.held_back ? 'Held back' : view.caps_now[0] ? `↑ ${(view.caps_now[0] / MB).toFixed(1)} MB/s cap` : '↑ No cap'} sub={view.conditions.metered ? 'Metered network' : view.conditions.on_battery ? 'On battery' : 'Unmetered, on power'} />
        </div>
        <div className="flex justify-end">
          <Button variant="ghost" disabled={!view.rows.length} onClick={() => void act(() => seedingApi.pauseAll(communityId, anyActive))}>
            {anyActive ? <Pause className="size-4" /> : <Play className="size-4" />} {anyActive ? 'Pause all' : 'Resume all'}
          </Button>
        </div>
        <div role="table" aria-label="Seeded files" className="flex flex-col">
          <div role="row" className="grid h-8 grid-cols-[minmax(0,2.4fr)_120px_64px_80px_140px_80px] items-center gap-s-2 text-meta font-semibold text-muted-foreground uppercase">
            <span>File</span>
            <span>Status</span>
            <span className="text-right">Ratio</span>
            <span className="text-right">Uploaded</span>
            <span>Seed limit</span>
            <span className="text-right">Actions</span>
          </div>
          {view.rows.map((r) => (
            <Row key={r.blob} row={r} open={open === r.blob} onToggle={() => setOpen(open === r.blob ? null : r.blob)} communityId={communityId} act={act} />
          ))}
          {!view.rows.length ? <p className="py-s-6 text-center text-body text-muted-foreground">This computer seeds nothing in this server yet.</p> : null}
        </div>
      </section>
      <SpeedPanel settings={view.settings} meteredKnown={view.conditions.metered_known} onSave={(s) => void act(() => seedingApi.setSettings(s))} />
    </div>
  );
}

function Tile({ k, v, sub }: { k: string; v: string; sub: string }) {
  return (
    <div className="rounded-lg bg-panel p-s-3">
      <div className="text-meta text-muted-foreground">{k}</div>
      <div className="text-head font-bold text-text-strong">{v}</div>
      <div className="text-meta text-muted-foreground">{sub}</div>
    </div>
  );
}

function Row({ row: r, open, onToggle, communityId, act }: { row: SeedRowView; open: boolean; onToggle: () => void; communityId: Id; act: (f: () => Promise<void>) => Promise<void> }) {
  const active = r.state === 'seeding' || r.state === 'scheduled' || r.state === 'queued';
  return (
    <div role="row" data-seed-row={r.blob} className={cn('flex flex-col border-t border-border', r.state === 'stopped' && 'opacity-60')}>
      <div className="grid h-14 grid-cols-[minmax(0,2.4fr)_120px_64px_80px_140px_80px] items-center gap-s-2 text-ui">
        <span className="flex min-w-0 items-center gap-s-2">
          <span className="grid h-8 w-10 shrink-0 place-items-center rounded text-[10px] font-bold text-white" style={tileStyle(r.kind)}>{r.ext || 'FILE'}</span>
          <span className="min-w-0">
            <span className="block truncate font-medium text-text-strong">{r.name}</span>
            <span className="block truncate text-meta text-muted-foreground">{r.folder ?? r.path ?? ''}</span>
          </span>
        </span>
        <span className="min-w-0">
          <span className={cn('inline-block rounded-full px-2 text-meta font-semibold', STATE_CLASS[r.state])} data-seed-state={r.state}>{STATE_LABEL[r.state]}</span>
          <span className="block truncate text-meta text-muted-foreground">{r.note}</span>
        </span>
        <span className={cn('text-right tabular-nums', r.ratio >= 1 ? 'text-success-text' : 'text-warn')}>{r.ratio.toFixed(1)}</span>
        <span className="text-right tabular-nums">{formatBytes(r.uploaded)}</span>
        <button type="button" aria-expanded={open} onClick={onToggle} className={cn('truncate rounded-md border px-s-2 py-1 text-left text-sm', open ? 'border-accent bg-selected' : 'border-border')}>
          {limitLabel(r.limit)}
        </button>
        <span className="flex justify-end gap-1">
          <Tip label={active ? 'Pause seeding' : 'Resume seeding'}>
            <Button variant="ghost" size="icon" aria-label={active ? 'Pause seeding' : 'Resume seeding'} onClick={() => void act(() => seedingApi.pause(communityId, r.blob, active))}>
              {active ? <Pause className="size-4" /> : <Play className="size-4" />}
            </Button>
          </Tip>
          <Tip label="Stop seeding">
            <Button variant="ghost" size="icon" aria-label="Stop seeding" disabled={r.state === 'stopped'} onClick={() => void act(() => seedingApi.stop(communityId, r.blob))}>
              <Square className="size-4" />
            </Button>
          </Tip>
        </span>
      </div>
      {open ? <LimitEditor row={r} onApply={(limit) => void act(async () => { await seedingApi.setLimit(communityId, r.blob, limit); onToggle(); })} onCancel={onToggle} /> : null}
    </div>
  );
}

function LimitEditor({ row, onApply, onCancel }: { row: SeedRowView; onApply: (limit: string) => void; onCancel: () => void }) {
  const [kind, value] = row.limit.split(':');
  const [mode, setMode] = useState(kind);
  const [ratio, setRatio] = useState(kind === 'ratio' ? (Number(value) / 1000).toString() : '5.0');
  const [hours, setHours] = useState(kind === 'hours' ? value : '48');
  // A week from the seed's start, when no date is set yet.
  const [date, setDate] = useState(() => new Date(kind === 'until' ? Number(value) : row.since_ms + 7 * 86_400_000).toISOString().slice(0, 10));
  const limit = mode === 'ratio' ? `ratio:${Math.round(Number(ratio) * 1000)}` : mode === 'hours' ? `hours:${Math.max(1, Math.round(Number(hours)))}` : mode === 'until' ? `until:${new Date(`${date}T23:59:59`).getTime()}` : 'forever';
  const option = (id: string, label: string, field?: React.ReactNode) => (
    <label className={cn('flex h-9 items-center gap-s-2 rounded-md px-s-2', mode === id && 'bg-hover')}>
      <input type="radio" name={`limit-${row.blob}`} checked={mode === id} onChange={() => setMode(id)} />
      <span className="flex-1">{label}</span>
      {field}
    </label>
  );
  return (
    <div role="dialog" aria-label="Seed this file for" data-limit-editor className="mb-s-2 ml-auto flex w-[320px] flex-col gap-1 rounded-lg border border-border bg-elevated p-s-2 text-ui">
      <span className="px-s-2 text-meta font-semibold text-muted-foreground uppercase">Seed this file for…</span>
      {option('forever', 'Forever')}
      {option('ratio', 'Until ratio', <Input aria-label="Ratio" className="h-7 w-20" value={ratio} onChange={(e) => setRatio(e.target.value)} />)}
      {option('hours', 'For', <Input aria-label="Hours" className="h-7 w-20" value={hours} onChange={(e) => setHours(e.target.value)} />)}
      {option('until', 'Until', <Input aria-label="Date" type="date" className="h-7 w-36" value={date} onChange={(e) => setDate(e.target.value)} />)}
      <div className="flex justify-end gap-s-2 pt-1">
        <Button variant="ghost" onClick={onCancel}>Cancel</Button>
        <Button onClick={() => onApply(limit)}>Apply</Button>
      </div>
    </div>
  );
}

function SpeedPanel({ settings, meteredKnown, onSave }: { settings: SeedingSettings; meteredKnown: boolean; onSave: (s: SeedingSettings) => void }) {
  const alt = settings.alt ?? { days: 0b001_1111, start_min: 9 * 60, end_min: 18 * 60, upload_bps: 2 * MB, download_bps: 10 * MB };
  const capRow = (label: string, value: number | null, set: (v: number | null) => void) => (
    <div className="flex items-center gap-s-2">
      <label className="flex flex-1 items-center gap-s-2 text-ui text-text-strong">
        <input type="checkbox" checked={value !== null} onChange={() => set(value === null ? 5 * MB : null)} />
        {label}
      </label>
      {value !== null ? (
        <Input aria-label={`${label} in MB/s`} type="number" min={0} className="h-8 w-20" value={(value / MB).toString()} onChange={(e) => set(Math.max(0, Number(e.target.value) || 0) * MB)} />
      ) : (
        <span className="text-sm text-muted-foreground">Unlimited</span>
      )}
    </div>
  );
  return (
    <aside aria-label="Speed and limits" data-speed className="flex min-h-0 flex-col gap-s-4 overflow-y-auto border-l border-border p-s-4">
      <h2 className="m-0 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">Speed &amp; limits</h2>
      {capRow('Upload limit', settings.upload_bps, (v) => onSave({ ...settings, upload_bps: v }))}
      {capRow('Download limit', settings.download_bps, (v) => onSave({ ...settings, download_bps: v }))}
      <div className="flex flex-col gap-s-2 rounded-lg bg-panel p-s-3">
        <label className="flex items-center gap-s-2 text-ui text-text-strong">
          <input type="checkbox" checked={settings.alt_on} onChange={() => onSave({ ...settings, alt_on: !settings.alt_on, alt })} />
          Alternative speed schedule
        </label>
        <div className={cn('flex flex-col gap-s-2', !settings.alt_on && 'opacity-40')}>
          <div className="flex gap-1" role="group" aria-label="Days">
            {DAYS.map((d, i) => {
              const on = (alt.days & (1 << i)) !== 0;
              return (
                <button
                  key={i}
                  type="button"
                  aria-pressed={on}
                  disabled={!settings.alt_on}
                  onClick={() => onSave({ ...settings, alt: { ...alt, days: alt.days ^ (1 << i) } })}
                  className={cn('size-7 rounded-md text-sm font-semibold', on ? 'bg-accent text-white' : 'bg-elevated text-muted-foreground')}
                >
                  {d}
                </button>
              );
            })}
          </div>
          <div className="flex items-center gap-s-2 text-sm">
            <Input aria-label="Starts" type="time" className="h-8 w-28" disabled={!settings.alt_on} value={hhmm(alt.start_min)} onChange={(e) => onSave({ ...settings, alt: { ...alt, start_min: minOf(e.target.value) } })} />
            –
            <Input aria-label="Ends" type="time" className="h-8 w-28" disabled={!settings.alt_on} value={hhmm(alt.end_min)} onChange={(e) => onSave({ ...settings, alt: { ...alt, end_min: minOf(e.target.value) } })} />
          </div>
          <div className="flex items-center gap-s-2 text-sm">
            Upload / download
            <Input aria-label="Scheduled upload MB/s" type="number" min={0} className="h-8 w-16" disabled={!settings.alt_on} value={((alt.upload_bps ?? 0) / MB).toString()} onChange={(e) => onSave({ ...settings, alt: { ...alt, upload_bps: Math.max(0, Number(e.target.value) || 0) * MB } })} />
            /
            <Input aria-label="Scheduled download MB/s" type="number" min={0} className="h-8 w-16" disabled={!settings.alt_on} value={((alt.download_bps ?? 0) / MB).toString()} onChange={(e) => onSave({ ...settings, alt: { ...alt, download_bps: Math.max(0, Number(e.target.value) || 0) * MB } })} />
            MB/s
          </div>
        </div>
      </div>
      <div className="flex items-center justify-between text-ui text-text-strong">
        Max active seeds
        <span className="flex items-center gap-1">
          <Button variant="ghost" size="icon" aria-label="Fewer" onClick={() => onSave({ ...settings, max_active: Math.max(1, settings.max_active - 1) })}><Minus className="size-4" /></Button>
          <span className="w-6 text-center tabular-nums" data-max-active>{settings.max_active}</span>
          <Button variant="ghost" size="icon" aria-label="More" onClick={() => onSave({ ...settings, max_active: Math.min(20, settings.max_active + 1) })}><Plus className="size-4" /></Button>
        </span>
      </div>
      <label className="flex items-start gap-s-2 text-ui text-text-strong">
        <input type="checkbox" checked={settings.unmetered_only} onChange={() => onSave({ ...settings, unmetered_only: !settings.unmetered_only })} />
        <span className="flex flex-col">
          Only seed on unmetered networks
          <span className="text-sm text-muted-foreground">
            {meteredKnown ? 'Pauses seeding on hotspots and metered Wi‑Fi.' : 'This computer cannot tell metered networks apart, so this never pauses seeding here.'}
          </span>
        </span>
      </label>
      <label className="flex items-start gap-s-2 text-ui text-text-strong">
        <input type="checkbox" checked={settings.pause_on_battery} onChange={() => onSave({ ...settings, pause_on_battery: !settings.pause_on_battery })} />
        <span className="flex flex-col">
          Pause when on battery
          <span className="text-sm text-muted-foreground">Resumes when you plug in.</span>
        </span>
      </label>
    </aside>
  );
}
