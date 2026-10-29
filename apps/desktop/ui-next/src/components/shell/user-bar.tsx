// The floating panel over the rail and the sidebar: your name and status, the mic and headphone buttons with their
// option chevrons, and settings. Clicking your name opens the status menu (a dropdown with duration submenus).
// Ported from the old UI's views/sidebar.js (renderMe) and the old UI's views/statusmenu.js.
import { useLayoutEffect, useRef, type CSSProperties } from 'react';
import { ChevronDown, Headphones, Mic, MicOff, PhoneOff, Settings } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { MOD, Tip } from '@/components/ui/tip';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { UserAvatar } from '@/components/user-avatar';
import { shortOf } from '@/lib/address';
import { has } from '@/lib/features';
import { useShell } from '@/lib/store';
import { openInputMenu, openOutputMenu } from '@/components/voice/voice-menus';
import type { Presence, StatusName } from '@/lib/types';
import { cn } from '@/lib/utils';

const MIN = 60 * 1000;
const DAY = 24 * 60 * MIN;

// Menu order. `presence` picks the glyph; timed statuses open the duration submenu.
const CHOICES: { status: StatusName; label: string; presence: Presence; timed: boolean; sub?: string }[] = [
  { status: 'online', label: 'Online', presence: 'online', timed: false },
  { status: 'idle', label: 'Idle', presence: 'idle', timed: true },
  { status: 'dnd', label: 'Do Not Disturb', presence: 'dnd', timed: true, sub: 'You will not receive desktop notifications' },
  { status: 'invisible', label: 'Invisible', presence: 'offline', timed: true, sub: 'You will appear offline' },
];

// Durations for a timed status. A null duration lasts until changed.
const DURATIONS: [string, number | null][] = [
  ['For 15 minutes', 15 * MIN],
  ['For 1 hour', 60 * MIN],
  ['For 8 hours', 8 * 60 * MIN],
  ['For 24 hours', DAY],
  ['For 3 days', 3 * DAY],
  ['Forever', null],
];

const choiceOf = (status: StatusName) => CHOICES.find((c) => c.status === status) ?? CHOICES[0];

// The glyph of a status in the menu. Each state keeps its own shape, as the sidebar dots do.
function MenuGlyph({ presence }: { presence: Presence }) {
  const shape: Record<Presence, string> = {
    online: 'bg-success',
    idle: 'bg-warn',
    dnd: 'bg-danger',
    offline: 'bg-transparent shadow-[inset_0_0_0_2px_var(--offline)]',
  };
  return <span aria-hidden="true" className={cn('inline-block size-2.5 shrink-0 rounded-full', shape[presence])} />;
}

function StatusMenu() {
  const me = useShell((s) => s.me);
  const status = useShell((s) => s.status);
  const call = useShell((s) => s.call);
  const setStatus = useShell((s) => s.setStatus);
  const choice = choiceOf(status.status);
  const name = me?.display_name || 'Set a display name';
  const statusText = call ? 'In a call' : choice.label;
  const trigger = (
    <button
      type="button"
      className="flex h-11 min-w-0 flex-1 items-center gap-s-2 rounded-lg px-1.5 text-left hover:bg-hover"
      aria-label={`${name}, ${statusText}. Profile and status`}
      aria-haspopup={has('status') ? 'menu' : undefined}
      title={me ? me.display_name : undefined}
      disabled={!has('status')}
    >
      <UserAvatar name={me?.display_name || '?'} seed={me?.root_id} presence={choice.presence} className="size-8" />
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-body leading-[19px] font-semibold text-text-strong">{name}</span>
        <span className="truncate text-meta leading-4 text-muted-foreground">{statusText}</span>
      </span>
    </button>
  );

  if (!has('status')) return trigger;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" className="w-[260px]">
        <DropdownMenuLabel className="flex items-center gap-s-2 font-normal">
          <UserAvatar name={me?.display_name || '?'} seed={me?.root_id} size="lg" presence={choice.presence} />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-body leading-[19px] font-semibold text-text-strong">{name}</span>
            {me ? <span className="truncate font-mono text-meta leading-4 text-muted-foreground">{shortOf('user', me.root_id)}</span> : null}
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {CHOICES.map((c) =>
          c.timed ? (
            <DropdownMenuSub key={c.status}>
              <DropdownMenuSubTrigger className="gap-s-2">
                <MenuGlyph presence={c.presence} />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span>{c.label}</span>
                  {c.sub ? <span className="text-meta font-normal text-muted-foreground">{c.sub}</span> : null}
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                {DURATIONS.map(([label, ms]) => (
                  <DropdownMenuItem key={label} onSelect={() => void setStatus(c.status, ms === null ? null : Date.now() + ms)}>
                    {label}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          ) : (
            <DropdownMenuItem key={c.status} className="gap-s-2" onSelect={() => void setStatus(c.status, null)}>
              <MenuGlyph presence={c.presence} />
              {c.label}
            </DropdownMenuItem>
          ),
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// Shown while you are in a call: the channel or call you are in, and a disconnect button.
function VoiceStrip() {
  const call = useShell((s) => s.call);
  const connection = useShell((s) => s.voiceConnection);
  const community = useShell((s) => s.community);
  const leaveCall = useShell((s) => s.leaveCall);
  if (!call) return null;
  const connected = connection && community?.id === connection.communityId
    ? community.channels.find((c) => c.id === connection.channelId)
    : null;
  const where = connected && community
    ? `#${connected.name} · ${community.name}`
    : (call.where ?? [call.channel_name, call.community_name].filter(Boolean).join(' / '));
  return (
    <div className="flex h-[60px] items-center gap-s-2 pr-s-2 pl-s-4 shadow-[inset_0_-1px_0_var(--border)]">
      <div className="min-w-0 flex-1">
        <div className="text-sm font-semibold text-success-text">Voice connected</div>
        <div className="truncate text-meta text-muted-foreground">{where}</div>
      </div>
      <Tip label="Disconnect">
        <Button variant="ghost" size="icon" className="rounded-md text-danger-text" aria-label="Disconnect" onClick={() => void leaveCall()}>
          <PhoneOff className="size-5" />
        </Button>
      </Tip>
    </div>
  );
}

export function UserBar() {
  const call = useShell((s) => s.call);
  const toggleMuted = useShell((s) => s.toggleMuted);
  const toggleDeafened = useShell((s) => s.toggleDeafened);
  const muted = Boolean(call?.muted);
  const deafened = Boolean(call?.deafened);
  const openSettings = useShell((s) => s.openSettings);
  // The status glyph shows on the panel, so the panel's ring is the panel's colour.
  const style = { '--presence-ring': 'var(--panel)' } as CSSProperties;
  // The panel floats over the bottom of the sidebar, and grows with the voice strip. Its height goes on the shell as
  // --user-panel-h, so the channel list can end above it instead of scrolling under it and showing through the gap.
  const ref = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const node = ref.current;
    const shell = node?.parentElement;
    if (!node || !shell) return;
    const measure = () => shell.style.setProperty('--user-panel-h', `${node.offsetHeight}px`);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => {
      observer.disconnect();
      shell.style.removeProperty('--user-panel-h');
    };
  }, []);
  return (
    <section
      ref={ref}
      aria-label="You"
      data-user-panel
      style={style}
      className="absolute bottom-s-2 left-s-2 z-[5] flex w-[306px] flex-col rounded-lg border border-border bg-panel shadow-[var(--shadow-1)]"
    >
      <VoiceStrip />
      <div className="flex h-[58px] items-center gap-s-1 pr-s-2 pl-[10px]">
        <StatusMenu />
        {has('calls') ? (
          <>
            {/* Each control sits flush with its options arrow, as one split button. */}
            {/* Mute and Deafen are disabled outside a call, so their tooltips hang on a wrapping span (Tip wrap). */}
            <div className="flex items-center">
              <Tip label={muted ? 'Unmute' : 'Mute'} wrap>
                <Button
                  variant="ghost"
                  size="icon"
                  className={cn('rounded-r-none text-text', muted && 'bg-selected text-text-strong')}
                  aria-label={muted ? 'Unmute' : 'Mute'}
                  aria-pressed={muted}
                  disabled={!call}
                  onClick={() => void toggleMuted()}
                >
                  {muted ? <MicOff className="size-5" /> : <Mic className="size-5" />}
                </Button>
              </Tip>
              <Tip label="Microphone options">
                <Button variant="ghost" size="icon" className="w-4 rounded-l-none text-muted-foreground" aria-label="Microphone options" aria-haspopup="menu" onClick={(e) => void openInputMenu(e.currentTarget)}>
                  <ChevronDown className="size-3.5" />
                </Button>
              </Tip>
            </div>
            <div className="flex items-center">
              <Tip label={deafened ? 'Undeafen' : 'Deafen'} wrap>
                <Button
                  variant="ghost"
                  size="icon"
                  className={cn('rounded-r-none text-text', deafened && 'bg-selected text-text-strong')}
                  aria-label={deafened ? 'Undeafen' : 'Deafen'}
                  aria-pressed={deafened}
                  disabled={!call}
                  onClick={() => void toggleDeafened()}
                >
                  <Headphones className="size-5" />
                </Button>
              </Tip>
              <Tip label="Output options">
                <Button variant="ghost" size="icon" className="w-4 rounded-l-none text-muted-foreground" aria-label="Output options" aria-haspopup="menu" onClick={(e) => void openOutputMenu(e.currentTarget)}>
                  <ChevronDown className="size-3.5" />
                </Button>
              </Tip>
            </div>
          </>
        ) : null}
        <Tip label="User Settings" shortcut={`${MOD}+,`}>
          <Button variant="ghost" size="icon" className="text-text" aria-label="Settings" onClick={() => openSettings()}>
            <Settings className="size-5" />
          </Button>
        </Tip>
      </div>
    </section>
  );
}
