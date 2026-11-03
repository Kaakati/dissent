// The file page's swarm: piece availability (each cell is 8 pieces of 64 KiB, coloured by how many online members hold
// all of them) and the members who hold the file, with how much and their level.
import { useEffect, useState } from 'react';
import { formatBytes } from '@/components/transfers/format';
import { filesApi, type ListingView, type LiveSwarm, type SwarmPeerView, type SwarmView } from '@/lib/files';
import type { Id } from '@/lib/types';
import { cn } from '@/lib/utils';

const PER_CELL = 8;
// The live figures are read from this device, so asking each second costs nothing on the network.
const LIVE_EVERY_MS = 1000;
// Who holds what comes from other devices: asked more often while something moves, and less often with many holders.
const SWARM_ACTIVE_MS = 5000;
const SWARM_PER_HOLDER_MS = 500;
const SWARM_IDLE_MS = 30000;

const rateOf = (bps: number) => `${formatBytes(bps)}/s`;

function etaOf(seconds: number): string {
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} s left`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min left`;
  return `${(seconds / 3600).toFixed(1)} h left`;
}

// The legend's buckets: 7+, 3–6, 1–2 (rare, fetched first), and nobody online.
function cellClass(n: number): string {
  if (n >= 7) return 'bg-[#23a55a]';
  if (n >= 3) return 'bg-[#1a7f43]';
  if (n >= 1) return 'bg-warn';
  return 'bg-elevated';
}

export function SwarmPanel({ communityId, listing }: { communityId: Id; listing: ListingView }) {
  const [swarm, setSwarm] = useState<SwarmView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState<LiveSwarm | null>(null);
  const active = now?.active ?? false;
  useEffect(() => {
    let live = true;
    const read = () => {
      filesApi
        .live(communityId, listing.id)
        .then((n) => {
          if (live) setNow(n);
        })
        .catch(() => {});
    };
    read();
    const timer = setInterval(read, LIVE_EVERY_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [communityId, listing.id]);
  const asked = swarm?.asked ?? 0;
  useEffect(() => {
    let live = true;
    const read = () => {
      filesApi
        .swarm(communityId, listing.id)
        .then((s) => {
          if (live) setSwarm(s);
        })
        .catch((err: unknown) => {
          if (live) setError(String(err));
        });
    };
    read();
    const timer = setInterval(read, active ? SWARM_ACTIVE_MS + asked * SWARM_PER_HOLDER_MS : SWARM_IDLE_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [communityId, listing.id, listing.seeders, listing.local, active, asked]);

  if (error) return <p className="m-0 text-sm text-muted-foreground">The swarm could not be read: {error}</p>;
  if (!swarm) return <p className="m-0 text-sm text-muted-foreground">Asking who has this file…</p>;
  // The pieces this device has fetched so far count at once; other holders' come with the slower swarm read.
  const pieces = swarm.pieces.slice();
  for (const [start, end] of now?.own_pieces ?? []) {
    for (let i = start; i < end && i < pieces.length; i++) pieces[i] += 1;
  }
  const cells: number[] = [];
  for (let i = 0; i < pieces.length; i += PER_CELL) {
    cells.push(Math.min(...pieces.slice(i, i + PER_CELL)));
  }
  const rates = new Map((now?.peers ?? []).map((p) => [p.root_id, p]));
  // Someone this device is sending to shows at once; the swarm read may not have heard of them yet.
  const peers: SwarmPeerView[] = [
    ...swarm.peers,
    ...(now?.peers ?? [])
      .filter((p) => !swarm.peers.some((s) => s.root_id === p.root_id))
      .map((p) => ({ root_id: p.root_id, you: false, name: p.name, role: 'member', storage: false, online: true, has: p.has, level: 0 })),
  ];
  const downloading = peers.filter((p) => Math.max(p.has, rates.get(p.root_id)?.has ?? 0) < 1).length;
  // Every holder this download asks has its upload slots full: it asks again shortly.
  const waiting = now?.progress != null && !now.down_bps && (now.peers ?? []).some((p) => p.busy);
  const facts = [
    waiting ? 'Waiting for a free upload slot' : '',
    now?.down_bps ? `↓ ${rateOf(now.down_bps)}` : '',
    now?.up_bps ? `↑ ${rateOf(now.up_bps)}` : '',
    now?.eta_s ? etaOf(now.eta_s) : '',
  ].filter(Boolean);
  return (
    <div className="flex max-w-[760px] flex-col gap-s-4">
      <section aria-label="Piece availability" data-pieces className="flex flex-col gap-s-2">
        <div className="flex items-baseline justify-between">
          <h3 className="m-0 text-ui font-semibold text-text-strong">Piece availability</h3>
          <span className="text-sm text-muted-foreground">
            {swarm.pieces.length.toLocaleString()} pieces of 64 KiB · each cell is {PER_CELL} pieces
          </span>
        </div>
        <div className="flex flex-wrap gap-[2px]" role="img" aria-label={`${cells.filter((n) => n > 0).length} of ${cells.length} cells held by someone online`}>
          {cells.map((n, i) => (
            <span key={i} className={cn('size-2.5 rounded-[2px]', cellClass(n))} />
          ))}
        </div>
        <div className="flex flex-wrap gap-s-4 text-meta text-muted-foreground">
          <span className="flex items-center gap-1"><span className="size-2.5 rounded-[2px] bg-[#23a55a]" />7+ members have it</span>
          <span className="flex items-center gap-1"><span className="size-2.5 rounded-[2px] bg-[#1a7f43]" />3–6</span>
          <span className="flex items-center gap-1"><span className="size-2.5 rounded-[2px] bg-warn" />1–2</span>
          <span className="flex items-center gap-1"><span className="size-2.5 rounded-[2px] bg-elevated" />Nobody online has it</span>
        </div>
      </section>
      <section aria-label="Swarm" data-swarm className="flex flex-col gap-s-2">
        <h3 className="m-0 text-ui font-semibold text-text-strong">
          Swarm <span className="font-normal text-muted-foreground">· {swarm.seeders} {swarm.seeders === 1 ? 'seeder' : 'seeders'} · {downloading} downloading</span>
          {facts.length ? <span data-swarm-speed className="ml-s-2 font-normal tabular-nums text-text">{facts.join(' · ')}</span> : null}
        </h3>
        <div role="table" aria-label="Members who have it" className="flex flex-col">
          <div role="row" className="grid h-8 grid-cols-[minmax(0,2fr)_80px_180px_80px] items-center text-meta font-semibold text-muted-foreground uppercase">
            <span>Member</span>
            <span className="text-right">Has</span>
            <span className="text-right">Speed</span>
            <span className="text-right">Level</span>
          </div>
          {peers.map((p) => {
            const moving = rates.get(p.root_id);
            const has = Math.max(p.has, moving?.has ?? 0);
            const speed = moving ? [moving.busy && !moving.from_bps ? 'Busy, asking again' : '', moving.from_bps ? `↓ ${rateOf(moving.from_bps)}` : '', moving.to_bps ? `↑ ${rateOf(moving.to_bps)}` : ''].filter(Boolean).join(' · ') : '';
            return (
            <div key={p.root_id} role="row" data-peer={p.you ? 'you' : undefined} className="grid h-11 grid-cols-[minmax(0,2fr)_80px_180px_80px] items-center border-t border-border text-ui">
              <span className="min-w-0">
                <span className="block truncate font-medium text-text-strong">{p.you ? 'You' : p.name}</span>
                <span className="block truncate text-meta text-muted-foreground">
                  {p.storage ? 'Storage member · encrypted copy only' : p.role === 'member' ? 'Member' : p.role[0].toUpperCase() + p.role.slice(1)}
                  {p.online ? '' : ' · offline'}
                </span>
              </span>
              <span className="text-right tabular-nums">{Math.round(has * 100)}%</span>
              <span className="text-right text-sm tabular-nums text-muted-foreground">{speed}</span>
              <span className="text-right text-muted-foreground">{p.storage ? 'Seeder' : `Lv ${p.level}`}</span>
            </div>
            );
          })}
          {!peers.length ? <p className="m-0 py-s-2 text-sm text-muted-foreground">Nobody else has said they hold it yet.</p> : null}
        </div>
      </section>
    </div>
  );
}
