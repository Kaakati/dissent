// The servers you share with someone: a mark with the server's picture or initials, and its name. The DM panel folds the
// list behind a count (mutual-summary.tsx); the full profile lists it. Mutual friends are never shown: contact lists stay private.
import { imageOf } from '@/lib/images';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import type { UserProfileView } from '@/lib/types';

type Mutual = UserProfileView['mutual_communities'][number];

// A server's mark at 24 px: its picture, or its initials on the colour its id gives.
export function ServerMark({ id, name }: { id: string; name: string }) {
  const src = imageOf(id);
  return (
    <span
      className="relative grid size-6 shrink-0 place-items-center overflow-hidden rounded-md text-[9px] font-bold text-white"
      style={{ background: hueBackground(hueOf(id)) }}
    >
      {src ? <img src={src} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : initials(name)}
    </span>
  );
}

export function MutualList({ servers }: { servers: Mutual[] }) {
  return (
    <ul className="flex flex-col gap-0.5">
      {servers.map((c) => (
        <li key={c.id} className="flex h-8 items-center gap-s-2 rounded-sm px-s-1 text-sm font-medium text-text hover:bg-hover">
          <ServerMark id={c.id} name={c.name} />
          <span className="truncate">{c.name}</span>
        </li>
      ))}
    </ul>
  );
}
