// The main column. The chat, friends, transfers and call boards are ported in later batches; until then this shows
// which view the current UI would show here, so the shell can be compared side by side with it.
import type { ReactNode } from 'react';
import { Users } from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';
import { DissentMark } from '@/components/dissent-mark';
import { UpdateButton } from '@/components/settings/update-dialog';
import { useShell, type ShellState } from '@/lib/store';

// The heading and the name of the view the current UI would show in this column.
function viewOf(s: Pick<ShellState, 'route' | 'channelId' | 'page' | 'peer' | 'peerName' | 'group' | 'community' | 'voiceChannelId'>): { title: string; view: string } {
  if (s.page === 'contacts') return { title: 'Friends', view: 'Friends' };
  if (s.page === 'transfers') return { title: 'Transfers', view: 'Transfers' };
  if (s.page === 'saved') return { title: 'Saved Messages', view: 'Saved Messages' };
  if (s.route.kind === 'server') {
    const channel = s.community?.channels.find((c) => c.id === s.channelId);
    if (s.voiceChannelId) return { title: channel?.name ?? '', view: 'Voice channel board' };
    if (channel) return { title: `#${channel.name}`, view: 'Channel chat' };
    return { title: s.community?.name ?? '', view: 'Server' };
  }
  if (s.group) return { title: s.group.name, view: 'Group chat' };
  if (s.peer) return { title: s.peerName, view: 'Direct message' };
  return { title: 'Direct Messages', view: 'Home' };
}

// `headerExtra` sits after the title in the header row (the Transfers tabs), and `children` is the body. A view with its
// own header (the chat) passes `header`, which replaces this row, and `overlay`, drawn over the whole column.
export function MainArea({ children, headerExtra, header, overlay }: { children?: ReactNode; headerExtra?: ReactNode; header?: ReactNode; overlay?: ReactNode }) {
  const place = useShell(
    useShallow((s) => ({
      route: s.route,
      channelId: s.channelId,
      page: s.page,
      peer: s.peer,
      peerName: s.peerName,
      group: s.group,
      community: s.community,
      voiceChannelId: s.voiceChannelId,
    })),
  );
  const { title, view } = viewOf(place);
  return (
    <main className="relative grid min-h-0 min-w-0 grid-rows-[56px_minmax(0,1fr)] bg-chat">
      {header ?? (
        <header className="flex items-center gap-s-2 px-s-4 shadow-[inset_0_-1px_0_var(--border)]">
          {place.page === 'contacts' ? <Users className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" /> : null}
          <h1 className="min-w-0 truncate text-head font-semibold text-text-strong">{title}</h1>
          {headerExtra}
          <div className="ml-auto flex shrink-0 items-center">
            <UpdateButton />
          </div>
        </header>
      )}
      {children ?? (
        <div className="flex min-h-0 flex-col items-center justify-center gap-s-3 p-s-8 text-center">
          <DissentMark className="size-12 text-faint" />
          <p className="text-body text-muted-foreground">Not ported yet: {view}.</p>
          <p className="text-sm text-faint">The current UI (apps/desktop/ui) shows it here.</p>
        </div>
      )}
      {overlay}
    </main>
  );
}
