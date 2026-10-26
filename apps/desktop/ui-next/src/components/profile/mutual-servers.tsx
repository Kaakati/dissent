// The servers you share with someone: a mark with the server's picture or initials, and its name. The DM panel folds the
// list behind a count; the full profile lists it. Mutual friends are never shown: contact lists stay private.
import { useState } from 'react';
import { Users } from 'lucide-react';
import { imageOf } from '@/lib/images';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import type { UserProfileView } from '@/lib/types';
import { cn } from '@/lib/utils';

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

// "N Mutual Servers" opens the list under the count. Disabled when there are none.
export function MutualToggle({ servers }: { servers: Mutual[] }) {
  const [open, setOpen] = useState(false);
  const none = servers.length === 0;
  return (
    <div className="flex flex-col">
      <button
        type="button"
        aria-expanded={open}
        disabled={none}
        onClick={() => setOpen((v) => !v)}
        className={cn('inline-flex h-8 items-center gap-s-2 self-start text-sm font-medium text-link', !none && 'hover:text-link-hover')}
      >
        <Users className="size-4" aria-hidden="true" />
        {`${servers.length} Mutual Server${servers.length === 1 ? '' : 's'}`}
      </button>
      {open && !none ? <MutualList servers={servers} /> : null}
    </div>
  );
}
