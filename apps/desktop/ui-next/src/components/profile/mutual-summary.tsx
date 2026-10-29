// The DM panel's mutuals as one muted line, as in the reference client: the friends' pictures, "N Mutual Friends", a dot
// and "N Mutual Servers". Each count still opens its list under the line. The friends part is left out when there are none.
import { useState } from 'react';
import type { MutualFriend, UserProfileView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { FriendList, FriendStack } from './mutual-friends';
import { MutualList } from './mutual-servers';

type Mutual = UserProfileView['mutual_communities'][number];

const PART = 'inline-flex items-center gap-1.5 text-caption font-medium text-muted-foreground';

export function MutualSummary({ friends, servers }: { friends: MutualFriend[]; servers: Mutual[] }) {
  const [open, setOpen] = useState<'friends' | 'servers' | null>(null);
  const toggle = (which: 'friends' | 'servers') => setOpen((v) => (v === which ? null : which));
  const friendsLabel = `${friends.length} Mutual Friend${friends.length === 1 ? '' : 's'}`;
  const noServers = servers.length === 0;
  return (
    <div className="flex flex-col gap-s-1" data-mutual-summary>
      <div className="flex min-h-6 flex-wrap items-center gap-x-s-2">
        {friends.length ? (
          <>
            <button type="button" aria-label={friendsLabel} aria-expanded={open === 'friends'} onClick={() => toggle('friends')} className={cn(PART, 'hover:text-text hover:underline')}>
              <FriendStack friends={friends} />
              {friendsLabel}
            </button>
            <span aria-hidden="true" className="text-faint">•</span>
          </>
        ) : null}
        <button type="button" aria-expanded={open === 'servers'} disabled={noServers} onClick={() => toggle('servers')} className={cn(PART, !noServers && 'hover:text-text hover:underline')}>
          {`${servers.length} Mutual Server${servers.length === 1 ? '' : 's'}`}
        </button>
      </div>
      {open === 'friends' ? <FriendList friends={friends} /> : null}
      {open === 'servers' && !noServers ? <MutualList servers={servers} /> : null}
    </div>
  );
}
