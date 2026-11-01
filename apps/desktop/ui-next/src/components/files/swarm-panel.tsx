// The file page's swarm: piece availability (each cell is 8 pieces of 64 KiB, coloured by how many online members hold
// all of them) and the members who hold the file, with how much and their level.
import { useEffect, useState } from 'react';
import { filesApi, type ListingView, type SwarmView } from '@/lib/files';
import type { Id } from '@/lib/types';
import { cn } from '@/lib/utils';

const PER_CELL = 8;

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
  useEffect(() => {
    let live = true;
    filesApi
      .swarm(communityId, listing.id)
      .then((s) => {
        if (live) setSwarm(s);
      })
      .catch((err: unknown) => {
        if (live) setError(String(err));
      });
    return () => {
      live = false;
    };
  }, [communityId, listing.id, listing.seeders, listing.local]);

  if (error) return <p className="m-0 text-sm text-muted-foreground">The swarm could not be read: {error}</p>;
  if (!swarm) return <p className="m-0 text-sm text-muted-foreground">Asking who has this file…</p>;
  const cells: number[] = [];
  for (let i = 0; i < swarm.pieces.length; i += PER_CELL) {
    cells.push(Math.min(...swarm.pieces.slice(i, i + PER_CELL)));
  }
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
          Swarm <span className="font-normal text-muted-foreground">· {swarm.seeders} seeders · {swarm.downloading} downloading</span>
        </h3>
        <div role="table" aria-label="Members who have it" className="flex flex-col">
          <div role="row" className="grid h-8 grid-cols-[minmax(0,2fr)_80px_80px] items-center text-meta font-semibold text-muted-foreground uppercase">
            <span>Member</span>
            <span className="text-right">Has</span>
            <span className="text-right">Level</span>
          </div>
          {swarm.peers.map((p) => (
            <div key={p.root_id} role="row" className="grid h-11 grid-cols-[minmax(0,2fr)_80px_80px] items-center border-t border-border text-ui">
              <span className="min-w-0">
                <span className="block truncate font-medium text-text-strong">{p.name}</span>
                <span className="block truncate text-meta text-muted-foreground">
                  {p.storage ? 'Storage member · encrypted copy only' : p.role === 'member' ? 'Member' : p.role[0].toUpperCase() + p.role.slice(1)}
                  {p.online ? '' : ' · offline'}
                </span>
              </span>
              <span className="text-right tabular-nums">{Math.round(p.has * 100)}%</span>
              <span className="text-right text-muted-foreground">{p.storage ? 'Seeder' : `Lv ${p.level}`}</span>
            </div>
          ))}
          {!swarm.peers.length ? <p className="m-0 py-s-2 text-sm text-muted-foreground">Nobody else has said they hold it yet.</p> : null}
        </div>
      </section>
    </div>
  );
}
