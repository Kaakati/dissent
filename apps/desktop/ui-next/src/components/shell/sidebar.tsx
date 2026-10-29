// The second column. Home: a find box, quick links and direct messages. A server: its name, quick links, and its
// channels and categories (channel-tree.tsx), with the people in each voice channel. Controls whose views are not ported yet are shown disabled.
// Ported from the old UI's views/sidebar.js.
import { useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { ArrowUpDown, Bookmark, BellOff, ChevronDown, Radio, Link as LinkIcon, Pin, Plus, UserPlus, Users } from 'lucide-react';
import { ChannelTree } from '@/components/shell/channel-tree';
import { rowClass, SectionHead, UnreadDot } from '@/components/shell/sidebar-rows';
import { openInviteDialog, openContactDialog } from '@/components/dialogs/invite-dialogs';
import { setSwitcherOpen } from '@/components/dialogs/quick-switcher';
import { openAddContactDialog } from '@/components/dialogs/small-dialogs';
import { NewConversationMenu, openConversationMenu } from '@/components/menus/conversation-menu';
import { ServerMenu } from '@/components/menus/server-menu';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { Separator } from '@/components/ui/separator';
import { UserAvatar } from '@/components/user-avatar';
import { bannerBackground, bannerOf, bannersStill, subscribeBannerMotion } from '@/lib/banners';
import { boostLabel } from '@/lib/boosts';
import { has } from '@/lib/features';
import { markdownPlain } from '@/lib/markdown';
import { plural } from '@/lib/identity';
import { useMutes } from '@/lib/mutes';
import { convKey, useShell } from '@/lib/store';
import type { CommunityView, ConversationSummary, GroupSummary, Presence } from '@/lib/types';
import { cn } from '@/lib/utils';

const NOT_PORTED = 'Not ported yet';

// Second line of a direct row for the presence states that have one worth saying.
const PRESENCE_TEXT: Partial<Record<Presence, string>> = { idle: 'Idle', dnd: 'Do Not Disturb' };

function Count({ value, label }: { value: number; label: string }) {
  return (
    <span aria-label={label} className="ml-auto grid h-4 min-w-4 place-items-center rounded-full bg-accent px-1 text-tiny font-bold text-white tabular-nums">
      {value}
    </span>
  );
}

function QuickLink({
  icon,
  label,
  onClick,
  extra,
  active = false,
  disabled = false,
}: {
  icon: ReactNode;
  label: string;
  onClick?: () => void;
  extra?: ReactNode;
  active?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={cn(rowClass(active, false), 'h-10')}
      aria-current={active ? 'page' : undefined}
      onClick={onClick}
      disabled={disabled}
      title={disabled ? NOT_PORTED : undefined}
    >
      <span className="shrink-0 text-muted-foreground [&_svg]:size-5">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {extra}
    </button>
  );
}

// ---- top of the column ----

function HomeTop() {
  return (
    <div className="flex h-14 shrink-0 items-center px-s-2 shadow-[inset_0_-1px_0_var(--border)]">
      <button
        type="button"
        aria-label="Find or start a conversation"
        aria-keyshortcuts="Control+K Meta+K"
        onClick={() => setSwitcherOpen(true)}
        className="flex h-10 w-full items-center justify-center truncate rounded-lg border border-border bg-rail px-s-2 text-sm font-medium text-muted-foreground hover:text-text"
      >
        Find or start a conversation
      </button>
    </div>
  );
}

const HEADER_PX = 56;

// The server name with its menu. With a banner, the banner fills the top of the column at 16:9 with the name over it,
// and it collapses into the plain header as the channel list scrolls, as Discord does. Without one, the plain header.
function ServerTop({ community, scrollTop }: { community: CommunityView; scrollTop: number }) {
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const background = bannerBackground(bannerOf(community.banner, community.banner_preset, community.banner_poster));
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const measure = () => setWidth(node.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const full = (width * 9) / 16;
  const height = background && width ? Math.max(HEADER_PX, full - scrollTop) : HEADER_PX;
  // 1 with the banner open, 0 once it has collapsed into the header.
  const open = background && full > HEADER_PX ? (height - HEADER_PX) / (full - HEADER_PX) : 0;
  const onBanner = open > 0.35;
  return (
    <div
      ref={ref}
      data-server-top={background ? 'banner' : 'plain'}
      className="relative shrink-0 overflow-hidden shadow-[inset_0_-1px_0_var(--border)]"
      style={{ height: background && !width ? undefined : height, aspectRatio: background && !width ? '16 / 9' : undefined }}
    >
      {background ? (
        <>
          <div data-server-banner className="absolute inset-0" style={{ background, opacity: open }} aria-hidden="true" />
          <div className="absolute inset-x-0 top-0 h-16 bg-gradient-to-b from-black/55 to-transparent" style={{ opacity: open }} aria-hidden="true" />
        </>
      ) : null}
      <div className="relative flex h-14 items-center px-s-2">
        <ServerMenu community={community}>
          <Tip label="Server options" side="bottom">
            <button
              type="button"
              aria-haspopup="menu"
              aria-label={`${community.name} menu`}
              className={cn(
                'flex h-10 w-full items-center justify-between gap-s-2 rounded-lg px-s-2 text-body font-semibold',
                onBanner ? 'text-white [text-shadow:0_1px_2px_rgb(0_0_0/0.5)] hover:bg-black/20' : 'text-text-strong hover:bg-hover',
              )}
            >
              {/* No native titles inside: the button's tooltip is the one that shows (the name is also the title bar's). */}
              <span data-server-name className="min-w-0 truncate">{community.name}</span>
              {community.boost?.level ? (
                <span
                  data-boost-level={community.boost.level}
                  aria-label={boostLabel(community.boost.level, community.boost.hosts.length)}
                  className={cn('ml-auto inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-1.5 text-meta font-semibold', onBanner ? 'bg-black/35 text-white' : 'bg-elevated text-text')}
                >
                  <Radio className="size-3" aria-hidden="true" />
                  {`Lv ${community.boost.level}`}
                </span>
              ) : null}
              <ChevronDown className={cn('size-4 shrink-0', onBanner ? 'text-white' : 'text-muted-foreground')} aria-hidden="true" />
            </button>
          </Tip>
        </ServerMenu>
      </div>
    </div>
  );
}

// ---- Home: direct messages ----

// Two small avatars, one over the other, for a group conversation. Sized as the current UI's stack avatar. The rail's
// large one is a 48 px circle over the rail's colour.
export function GroupAvatar({ conv, large = false }: { conv: GroupSummary; large?: boolean }) {
  const [a, b] = conv.members.slice(0, 2);
  const part = large ? 'size-7 text-[9px]' : 'size-5 text-[7px]';
  return (
    <span className={cn('relative shrink-0', large ? 'size-12' : 'size-8')}>
      {a ? <UserAvatar name={a.display_name} seed={a.root_id} size="xs" className={cn('absolute top-0 left-0', part)} /> : null}
      {b ? (
        <UserAvatar
          name={b.display_name}
          seed={b.root_id}
          size="xs"
          className={cn('absolute right-0 bottom-0', part, large ? 'shadow-[0_0_0_2px_var(--rail)]' : 'shadow-[0_0_0_2px_var(--sidebar)]')}
        />
      ) : null}
    </span>
  );
}

function ConversationRow({ conv }: { conv: ConversationSummary }) {
  const route = useShell((s) => s.route);
  const page = useShell((s) => s.page);
  const peer = useShell((s) => s.peer);
  const unreadSet = useShell((s) => s.unread);
  const openConversation = useShell((s) => s.openConversation);
  const muted = useMutes((s) => convKey(conv) in s.mutes);
  const count = conv.unread ?? 0;
  // Unread mentions show in red, as a channel's do, and through a mute, since a mention alerts through one.
  const mentions = has('dm_mentions') ? (conv.mentions ?? 0) : 0;

  const id = conv.kind === 'group' ? conv.id : conv.peer;
  const active = route.kind === 'home' && !page && peer === id;
  const unread = unreadSet.has(convKey(conv)) && !active;
  const name = conv.kind === 'group' ? conv.name : conv.display_name;
  const live = conv.live_call ?? null;
  const presence: Presence | undefined = conv.kind === 'dm'
    ? (conv.status ?? (typeof conv.online === 'boolean' ? (conv.online ? 'online' : 'offline') : undefined))
    : undefined;
  const sub = live
    ? (live.scope === 'dm' ? 'In a call with you' : `Call · ${live.participants?.length ?? 0} in`)
    : conv.kind === 'group'
      ? plural(conv.members.length, 'member')
      : ((presence && PRESENCE_TEXT[presence]) ?? (conv.last_text === undefined ? undefined : markdownPlain(conv.last_text)) ?? '');

  return (
    <button
      type="button"
      className={cn(rowClass(active, unread), 'h-[42px] gap-s-3 text-body')}
      aria-current={active ? 'true' : undefined}
      title={name}
      onClick={() => void openConversation(conv)}
      onContextMenu={(e) => {
        e.preventDefault();
        openConversationMenu(e.clientX, e.clientY, conv);
      }}
    >
      {unread ? <UnreadDot /> : null}
      {conv.kind === 'group' ? <GroupAvatar conv={conv} /> : <UserAvatar name={name} seed={conv.peer} presence={presence} />}
      <span className="flex min-w-0 flex-1 flex-col">
        <span className={cn('truncate leading-5', (active || unread) && 'text-text-strong')}>{name}</span>
        {sub ? (
          <span className={cn('truncate text-meta leading-4 font-normal text-muted-foreground', live && 'text-success-text')}>{sub}</span>
        ) : null}
      </span>
      {mentions > 0 && !active ? (
        <span aria-label={`${mentions} ${mentions === 1 ? 'mention' : 'mentions'}`} className="ml-auto grid h-4 min-w-4 shrink-0 place-items-center rounded-full bg-danger px-1 text-tiny leading-none font-bold text-white">
          {mentions > 99 ? '99+' : mentions}
        </span>
      ) : count > 0 && !active && !muted ? (
        <span aria-label={`${count} unread`} className="ml-auto grid h-4 min-w-4 shrink-0 place-items-center rounded-full bg-accent px-1 text-tiny leading-none font-bold text-white">
          {count > 99 ? '99+' : count}
        </span>
      ) : null}
      {conv.pinned ? <Pin className="ml-auto size-4 shrink-0 text-muted-foreground" aria-label="Pinned" /> : null}
      {muted ? <BellOff className={cn('size-4 shrink-0 text-muted-foreground', !conv.pinned && 'ml-auto')} aria-label="Muted" /> : null}
    </button>
  );
}

function ActiveBadge({ count }: { count: number }) {
  return <span className="ml-auto inline-flex h-[18px] items-center rounded-full bg-accent-tint px-1.5 text-tiny font-semibold text-accent-text">{count} active</span>;
}

function HomeList() {
  const conversations = useShell((s) => s.conversations);
  const page = useShell((s) => s.page);
  const contacts = useShell((s) => s.contacts);
  const transfers = useShell((s) => s.transfers);
  const openPage = useShell((s) => s.openPage);
  const incoming = contacts?.incoming?.length ?? 0;
  const activeTransfers = transfers.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)).length;
  return (
    <>
      {has('contacts') ? (
        <QuickLink
          icon={<Users />}
          label="Friends"
          onClick={() => void openPage('contacts')}
          active={page === 'contacts'}
          extra={incoming ? <Count value={incoming} label={`${incoming} contact requests`} /> : null}
        />
      ) : null}
      <QuickLink icon={<LinkIcon />} label="Invite links" onClick={() => openContactDialog()} />
      {has('transfers') ? (
        <QuickLink
          icon={<ArrowUpDown />}
          label="Transfers"
          onClick={() => void openPage('transfers')}
          active={page === 'transfers'}
          extra={activeTransfers ? <ActiveBadge count={activeTransfers} /> : null}
        />
      ) : null}
      <QuickLink icon={<Bookmark />} label="Saved Messages" onClick={() => void openPage('saved')} active={page === 'saved'} />
      <Separator className="my-s-2" />
      <SectionHead
        plain
        title="Direct Messages"
        action={
          has('groups') ? (
            <NewConversationMenu>
              <Tip label="Create DM">
                <Button variant="ghost" size="icon" aria-label="New conversation" aria-haspopup="menu">
                  <Plus className="size-4" />
                </Button>
              </Tip>
            </NewConversationMenu>
          ) : (
            <Tip label="Add contact">
              <Button variant="ghost" size="icon" aria-label="Add contact" onClick={() => openAddContactDialog()}>
                <Plus className="size-4" />
              </Button>
            </Tip>
          )
        }
      />
      {conversations.length ? (
        conversations.map((c) => <ConversationRow key={convKey(c)} conv={c} />)
      ) : (
        <p className="p-s-2 text-sm text-muted-foreground">No conversations yet. Add a contact to start one.</p>
      )}
    </>
  );
}

// ---- server: channels and voice ----

function ServerList({ community }: { community: CommunityView }) {
  const page = useShell((s) => s.page);
  const transfers = useShell((s) => s.transfers);
  const openPage = useShell((s) => s.openPage);
  const activeTransfers = transfers.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)).length;
  const canManage = community.my_role === 'owner' || community.my_role === 'admin';
  return (
    <>
      <QuickLink icon={<UserPlus />} label="Invite people" onClick={() => openInviteDialog(community.id)} />
      {has('transfers') ? (
        <QuickLink icon={<ArrowUpDown />} label="Transfers" onClick={() => void openPage('transfers')} active={page === 'transfers'} extra={activeTransfers ? <ActiveBadge count={activeTransfers} /> : null} />
      ) : null}
      <Separator className="my-s-2" />
      <ChannelTree community={community} canManage={canManage} />
    </>
  );
}

// Placeholder rows while a server's view loads.
function SkeletonRows() {
  return (
    <div className="flex flex-col gap-0.5" aria-hidden="true">
      {[55, 70, 45, 60, 50].map((w) => (
        <div key={w} className="flex h-9 items-center px-s-2">
          <span className="h-3.5 rounded-sm bg-elevated" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}

// The column: the top, then the scrolling list. The list stops above the user panel, so no row is ever under it.
export function Sidebar() {
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  // The channel list's scroll, which collapses a server banner. A new server starts at the top.
  const [scrollTop, setScrollTop] = useState(0);
  const [scrolledFor, setScrolledFor] = useState(community?.id);
  if (scrolledFor !== community?.id) {
    setScrolledFor(community?.id);
    setScrollTop(0);
  }
  return (
    <aside aria-label="Channels and conversations" className="flex min-h-0 flex-col overflow-hidden border-l border-divider bg-sidebar">
      {route.kind === 'home' ? <HomeTop /> : null}
      {route.kind === 'server' && community ? <ServerTop community={community} scrollTop={scrollTop} /> : null}
      {route.kind === 'server' && !community ? <div className="h-14 shrink-0 px-s-2 pt-4"><span className="block h-6 w-[70%] rounded-sm bg-elevated" /></div> : null}
      {/* The list ends a gap above the floating user panel (user-bar.tsx sets --user-panel-h), so no row shows under it. */}
      <div
        data-sidebar-list
        className="dissent-scroll mb-[calc(var(--user-panel-h,60px)+var(--s-2)*2)] flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-s-2"
        onScroll={route.kind === 'server' ? (e) => setScrollTop(e.currentTarget.scrollTop) : undefined}
      >
        {route.kind === 'home' ? <HomeList /> : null}
        {route.kind === 'server' && community ? <ServerList community={community} /> : null}
        {route.kind === 'server' && !community ? <SkeletonRows /> : null}
      </div>
    </aside>
  );
}
