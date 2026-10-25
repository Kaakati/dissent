// The far-left column: Home (direct messages) on top, one round button per server, then "+" at the end of the list.
// The active place has a pill on its left edge (shell.css). Each button names itself in a tooltip to the right, as the
// current UI's data-tip labels do. Ported from ui/views/rail.js.
import type { ReactNode } from 'react';
import { Plus } from 'lucide-react';
import { DissentMark } from '@/components/dissent-mark';
import { openServerDialog } from '@/components/dialogs/server-dialog';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { imageOf } from '@/lib/images';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';

function Labelled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="right" sideOffset={16}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}

export function ServerRail() {
  const route = useShell((s) => s.route);
  const communities = useShell((s) => s.communities);
  const unread = useShell((s) => s.unread);
  const openHome = useShell((s) => s.openHome);
  const openServer = useShell((s) => s.openServer);

  const onHome = route.kind === 'home';
  // Unread conversations (direct and group) make the Home badge; the number is the count of them.
  const dmUnread = [...unread].filter((k) => k.startsWith('dm:') || k.startsWith('group:')).length;

  return (
    <nav aria-label="Servers and direct messages" className="flex flex-col items-center gap-[var(--rail-gap)] overflow-visible bg-rail pt-s-1 pb-[82px]">
      <div className={cn('rail-item relative grid h-12 w-[72px] shrink-0 place-items-center', onHome && 'active')}>
        <Labelled label="Direct Messages">
          <Button
            variant="ghost"
            size="icon"
            aria-label="Direct messages"
            aria-current={onHome ? 'page' : undefined}
            onClick={() => void openHome()}
            className={cn('size-12 rounded-3xl bg-chat text-brand hover:rounded-2xl hover:bg-accent hover:text-white', onHome && 'rounded-2xl bg-accent text-white')}
          >
            <DissentMark className="size-6" />
          </Button>
        </Labelled>
        {dmUnread > 0 ? <Badge count={dmUnread} label={`${dmUnread} unread conversations`} /> : null}
      </div>

      <hr className="m-0 h-0.5 w-8 shrink-0 rounded-sm border-0 bg-chat" />

      {communities.map((c) => {
        const active = route.kind === 'server' && route.communityId === c.id;
        const hasUnread = [...unread].some((k) => k.startsWith(`ch:${c.id}:`));
        const image = imageOf(c.id);
        return (
          <div key={c.id} className={cn('rail-item relative grid h-12 w-[72px] shrink-0 place-items-center', active && 'active', hasUnread && 'unread')}>
            <Labelled label={c.name}>
              <button
                type="button"
                role="tab"
                aria-selected={active}
                aria-current={active ? 'page' : undefined}
                aria-label={c.name}
                onClick={() => void openServer(c.id)}
                className={cn(
                  'relative grid size-12 place-items-center overflow-hidden rounded-3xl border-0 p-0 text-[15px] leading-none font-semibold text-white transition-[border-radius,background-color] duration-150 hover:rounded-2xl',
                  active && 'rounded-2xl',
                )}
                style={{ background: hueBackground(c.hue ?? hueOf(c.id)) }}
              >
                {initials(c.name)}
                {image ? <img src={image} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : null}
              </button>
            </Labelled>
            {typeof c.mentions === 'number' && c.mentions > 0 ? <Badge count={c.mentions} label={`${c.mentions} mentions`} /> : null}
          </div>
        );
      })}

      <div className="rail-item relative grid h-12 w-[72px] shrink-0 place-items-center">
        <Labelled label="Add a Server">
          <Button
            variant="ghost"
            size="icon"
            aria-label="Create or join a server"
            onClick={() => openServerDialog()}
            className="size-12 rounded-3xl bg-chat text-accent-text hover:rounded-2xl hover:bg-accent hover:text-white disabled:opacity-100"
          >
            <Plus className="size-6" />
          </Button>
        </Labelled>
      </div>
    </nav>
  );
}

function Badge({ count, label }: { count: number; label: string }) {
  return (
    <span
      aria-label={label}
      className="absolute right-2 -bottom-0.5 z-[2] grid h-4 min-w-4 place-items-center rounded-full bg-accent px-1 text-tiny font-bold text-white shadow-[0_0_0_2px_var(--rail)] tabular-nums"
    >
      {count}
    </span>
  );
}
