// The contacts you and someone else both have. A stack of their pictures and "N Mutual Friends" opens the list, as the
// mutual servers count does. Shown only when there are some: the profile never says whether a person shares their list.
import { useState } from 'react';
import { Users } from 'lucide-react';
import { UserAvatar } from '@/components/user-avatar';
import type { MutualFriend } from '@/lib/types';
import { cn } from '@/lib/utils';

// Up to three pictures, overlapping, the first one on top.
export function FriendStack({ friends }: { friends: MutualFriend[] }) {
  return (
    <span className="flex items-center" aria-hidden="true">
      {friends.slice(0, 3).map((f, i) => (
        <UserAvatar
          key={f.root_id}
          name={f.display_name}
          seed={f.root_id}
          size="xs"
          className={cn('ring-2 ring-panel', i > 0 && '-ml-1.5')}
        />
      ))}
    </span>
  );
}

export function FriendList({ friends }: { friends: MutualFriend[] }) {
  return (
    <ul className="flex flex-col gap-0.5">
      {friends.map((f) => (
        <li key={f.root_id} className="flex h-8 items-center gap-s-2 rounded-sm px-s-1 text-sm font-medium text-text hover:bg-hover">
          <UserAvatar name={f.display_name} seed={f.root_id} size="sm" />
          <span className="truncate">{f.display_name}</span>
          <span className="ml-auto font-mono text-meta text-muted-foreground">{`·${f.suffix}`}</span>
        </li>
      ))}
    </ul>
  );
}

// "N Mutual Friends": the count and a stack of pictures. Clicking it lists them. Renders nothing when there are none.
export function MutualFriendsToggle({ friends }: { friends: MutualFriend[] }) {
  const [open, setOpen] = useState(false);
  if (friends.length === 0) return null;
  const n = friends.length;
  const label = `${n} Mutual Friend${n === 1 ? '' : 's'}`;
  return (
    <div className="flex flex-col">
      <button
        type="button"
        aria-label={label}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex h-8 items-center gap-s-2 self-start text-sm font-medium text-link hover:text-link-hover"
      >
        <FriendStack friends={friends} />
        <Users className="size-4" aria-hidden="true" />
        {label}
      </button>
      {open ? <FriendList friends={friends} /> : null}
    </div>
  );
}
