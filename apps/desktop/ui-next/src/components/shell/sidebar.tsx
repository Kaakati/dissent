// The second column. Home: a find box, quick links and direct messages. A server: its name, quick links, text
// channels and voice channels with the people in them. Controls whose views are not ported yet are shown disabled.
// Ported from the old UI's views/sidebar.js.
import { Fragment, useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { ArrowDown, ArrowUp, ArrowUpDown, Bookmark, BellOff, ChevronDown, Hash, Radio, Headphones, Link as LinkIcon, Lock, MicOff, Pin, Plus, Settings, UserPlus, Users, Volume2 } from 'lucide-react';
import { ChannelDropLine, moveChannel, useChannelReorder } from '@/components/shell/channel-reorder';
import { openChannelSettings } from '@/components/channel-settings/channel-settings';
import { openCreateChannelDialog } from '@/components/dialogs/create-channel-dialog';
import { openInviteDialog, openContactDialog } from '@/components/dialogs/invite-dialogs';
import { setSwitcherOpen } from '@/components/dialogs/quick-switcher';
import { openContextMenu } from '@/lib/menu';
import { openAddContactDialog } from '@/components/dialogs/small-dialogs';
import { openVoiceChannel } from '@/lib/calls';
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
import { DEFAULT_PREFS, useServerPrefs } from '@/lib/server-prefs';
import { channelKey, convKey, useShell } from '@/lib/store';
import type { CommunityView, ConversationSummary, GroupSummary, Id, Presence, VoiceParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';

const NOT_PORTED = 'Not ported yet';

// Second line of a direct row for the presence states that have one worth saying.
const PRESENCE_TEXT: Partial<Record<Presence, string>> = { idle: 'Idle', dnd: 'Do Not Disturb' };

const rowClass = (active: boolean, unread: boolean) =>
  cn(
    'relative flex h-9 w-full min-w-0 shrink-0 items-center gap-s-2 rounded-lg px-s-2 text-left text-ui font-medium text-muted-foreground hover:bg-hover hover:text-text disabled:pointer-events-none disabled:opacity-50',
    active && 'bg-selected text-text-strong',
    unread && 'font-semibold text-text-strong',
  );

function UnreadDot() {
  return <span aria-label="unread" className="absolute top-3.5 -left-2 h-2 w-1 rounded-r bg-text-strong" />;
}

function Count({ value, label }: { value: number; label: string }) {
  return (
    <span aria-label={label} className="ml-auto grid h-4 min-w-4 place-items-center rounded-full bg-accent px-1 text-tiny font-bold text-white tabular-nums">
      {value}
    </span>
  );
}

// A section's heading. The channel sections are small caps; the direct messages heading is title case, as the reference shows it.
function SectionHead({ title, action, spaced = false, plain = false }: { title: string; action?: ReactNode; spaced?: boolean; plain?: boolean }) {
  return (
    <div
      className={cn(
        'flex h-8 shrink-0 items-center justify-between pl-s-2 text-muted-foreground',
        plain ? 'text-[13px] font-normal' : 'text-meta font-semibold tracking-[.02em] uppercase',
        spaced && 'mt-s-2',
      )}
    >
      <span className="truncate">{title}</span>
      {action}
    </div>
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

// Moving a channel one place from its menu: the place it has among the shown rows of its kind, how many there are, and
// the move. Absent when the server's channel order cannot be changed here.
interface MenuMove {
  index: number;
  count: number;
  place: (to: number) => void;
}

// The owner's actions on a channel row, as a menu: right click, or Shift+F10 on the focused row. The same two actions
// are the hover and focus buttons of ChannelActions, so a pointer-less user reaches them too. Move Up and Move Down
// are the menu's form of Alt+Shift+Up and Down.
function openChannelMenu(e: { preventDefault: () => void; clientX: number; clientY: number; currentTarget: HTMLElement }, communityId: Id, channelId: Id, move?: MenuMove): void {
  e.preventDefault();
  // A keyboard menu key sends no pointer position, so the menu opens by the row.
  const r = e.currentTarget.getBoundingClientRect();
  openContextMenu(e.clientX || r.left + 20, e.clientY || r.bottom, [
    { label: 'Create invite', icon: UserPlus, onPick: () => openInviteDialog(communityId) },
    { label: 'Edit Channel', icon: Settings, onPick: () => openChannelSettings(communityId, channelId) },
    move ? 'sep' : null,
    move ? { label: 'Move Up', icon: ArrowUp, disabled: move.index === 0, onPick: () => move.place(move.index - 1) } : null,
    move ? { label: 'Move Down', icon: ArrowDown, disabled: move.index >= move.count - 1, onPick: () => move.place(move.index + 1) } : null,
  ]);
}

// The owner's hover actions on a channel row: Create invite and Edit Channel. They show on hover and focus.
function ChannelActions({ communityId, channelId }: { communityId: Id; channelId: Id }) {
  const button = 'grid size-6 place-items-center rounded-sm bg-elevated text-muted-foreground hover:bg-elevated-hover hover:text-text-strong';
  return (
    <span className="absolute top-1/2 right-1 hidden -translate-y-1/2 items-center gap-0.5 group-focus-within:flex group-hover:flex">
      <Tip label="Create Invite">
        <button type="button" aria-label="Create invite" onClick={() => openInviteDialog(communityId)} className={button}>
          <UserPlus className="size-3.5" />
        </button>
      </Tip>
      <Tip label="Edit Channel">
        <button type="button" aria-label="Edit Channel" onClick={() => openChannelSettings(communityId, channelId)} className={button}>
          <Settings className="size-3.5" />
        </button>
      </Tip>
    </span>
  );
}

function ServerList({ community }: { community: CommunityView }) {
  const channelId = useShell((s) => s.channelId);
  const page = useShell((s) => s.page);
  const voiceChannelId = useShell((s) => s.voiceChannelId);
  const unread = useShell((s) => s.unread);
  const transfers = useShell((s) => s.transfers);
  const openChannel = useShell((s) => s.openChannel);
  const openPage = useShell((s) => s.openPage);
  const activeTransfers = transfers.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)).length;
  const canManage = community.my_role === 'owner' || community.my_role === 'admin';
  const canReorder = canManage && has('channel_order');
  const prefs = useServerPrefs((s) => s.prefs[community.id]) ?? DEFAULT_PREFS;
  const mutes = useMutes((s) => s.mutes);
  // Hide Muted Channels hides a muted text channel, except the one open now, so the user keeps their place. As the DM
  // rows do, a timed mute that has run out is still listed until a check removes it.
  const text = community.channels.filter((c) => !c.voice && !(prefs.hide_muted_channels && channelKey(community.id, c.id) in mutes && c.id !== channelId));
  const shown = text.map((c) => c.id);
  const reorder = useChannelReorder(canReorder, text, (id, to) => moveChannel(community, shown, id, to));
  return (
    <>
      <QuickLink icon={<UserPlus />} label="Invite people" onClick={() => openInviteDialog(community.id)} />
      {has('transfers') ? (
        <QuickLink icon={<ArrowUpDown />} label="Transfers" onClick={() => void openPage('transfers')} active={page === 'transfers'} extra={activeTransfers ? <ActiveBadge count={activeTransfers} /> : null} />
      ) : null}
      <Separator className="my-s-2" />
      <SectionHead
        title="Text channels"
        action={
          canManage ? (
            <Tip label="Create Channel">
              <Button variant="ghost" size="icon" aria-label="Create channel" onClick={() => openCreateChannelDialog(community.id)}>
                <Plus className="size-4" />
              </Button>
            </Tip>
          ) : null
        }
      />
      {text.length ? (
        text.map((ch, index) => {
          const active = ch.id === channelId && !voiceChannelId && !page;
          const isUnread = unread.has(`ch:${community.id}:${ch.id}`) && !active;
          const move = canReorder ? { index, count: text.length, place: (to: number) => reorder.place(ch.id, to) } : undefined;
          return (
            <Fragment key={ch.id}>
            {reorder.lineAt(index) ? <ChannelDropLine /> : null}
            <div
              ref={reorder.rowRef(ch.id)}
              style={reorder.rowStyle(ch.id)}
              className={cn('group relative flex shrink-0 flex-col', reorder.dragging(ch.id) && 'z-[3] opacity-80')}
            >
            <button
              type="button"
              className={cn(rowClass(active, isUnread), canReorder && 'touch-none')}
              aria-current={active ? 'true' : undefined}
              aria-keyshortcuts={canReorder ? 'Alt+Shift+ArrowUp Alt+Shift+ArrowDown' : undefined}
              onClick={() => {
                if (reorder.clickOpens()) void openChannel(ch.id);
              }}
              onContextMenu={canManage ? (e) => openChannelMenu(e, community.id, ch.id, move) : undefined}
              {...reorder.handlers(ch.id, index)}
            >
              {isUnread ? <UnreadDot /> : null}
              {ch.private ? (
                <Lock className="size-5 shrink-0 text-muted-foreground" aria-label="Private channel" role="img" />
              ) : (
                <Hash className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
              )}
              <span className="min-w-0 flex-1 truncate" title={`#${ch.name}`}>{ch.name}</span>
              {has('mentions') && ch.mentions ? (
                <span aria-label={`${ch.mentions} mentions`} className="ml-auto grid h-4 min-w-4 shrink-0 place-items-center rounded-full bg-danger px-1 text-[10px] leading-none font-bold text-white">{ch.mentions}</span>
              ) : null}
            </button>
            {canManage ? <ChannelActions communityId={community.id} channelId={ch.id} /> : null}
            </div>
            </Fragment>
          );
        })
      ) : (
        <p className="p-s-2 text-sm text-muted-foreground">No channels yet.</p>
      )}
      {reorder.lineAt(text.length) ? <ChannelDropLine /> : null}
      <div role="status" aria-live="polite" className="sr-only" data-channel-order-status="text">
        {reorder.announcement}
      </div>
      {has('calls') ? <VoiceSection community={community} canManage={canManage} canReorder={canReorder} /> : null}
    </>
  );
}

function VoiceSection({ community, canManage, canReorder }: { community: CommunityView; canManage: boolean; canReorder: boolean }) {
  const voiceChannelId = useShell((s) => s.voiceChannelId);
  const connection = useShell((s) => s.voiceConnection);
  const me = useShell((s) => s.me);
  const call = useShell((s) => s.call);
  const remote = community.voice_channels ?? [];
  const channels = community.channels.filter((c) => c.voice);
  const shown = channels.map((c) => c.id);
  const reorder = useChannelReorder(canReorder, channels, (id, to) => moveChannel(community, shown, id, to));
  return (
    <>
      <SectionHead
        spaced
        title="Voice channels"
        action={
          canManage ? (
            <Tip label="Create Voice Channel">
              <Button variant="ghost" size="icon" aria-label="Create voice channel" onClick={() => openCreateChannelDialog(community.id, 'voice')}>
                <Plus className="size-4" />
              </Button>
            </Tip>
          ) : null
        }
      />
      {channels.map((ch, index) => {
        const active = voiceChannelId === ch.id;
        const connected = connection?.channelId === ch.id;
        const others: VoiceParticipant[] = remote.find((v) => v.id === ch.id)?.participants ?? [];
        // The engine lists everyone but this device; the channel you are connected to lists you first.
        const participants: VoiceParticipant[] = connected && me && call
          ? [{ root_id: me.root_id, display_name: me.display_name, muted: call.muted, deafened: call.deafened, speaking: false }, ...others]
          : others;
        const move = canReorder ? { index, count: channels.length, place: (to: number) => reorder.place(ch.id, to) } : undefined;
        return (
          <Fragment key={ch.id}>
          {reorder.lineAt(index) ? <ChannelDropLine /> : null}
          <div
            ref={reorder.rowRef(ch.id)}
            style={reorder.rowStyle(ch.id)}
            className={cn('group relative flex flex-col gap-0.5', reorder.dragging(ch.id) && 'z-[3] opacity-80')}
          >
            <button
              type="button"
              className={cn(rowClass(active, false), 'font-medium', canReorder && 'touch-none')}
              aria-current={active ? 'true' : undefined}
              aria-keyshortcuts={canReorder ? 'Alt+Shift+ArrowUp Alt+Shift+ArrowDown' : undefined}
              title={connected ? `Connected to ${ch.name}` : `Join ${ch.name}`}
              onClick={() => {
                if (reorder.clickOpens()) void openVoiceChannel(ch.id);
              }}
              onContextMenu={canManage ? (e) => openChannelMenu(e, community.id, ch.id, move) : undefined}
              {...reorder.handlers(ch.id, index)}
            >
              <Volume2 className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate" title={ch.name}>{ch.name}</span>
            </button>
            {canManage ? <ChannelActions communityId={community.id} channelId={ch.id} /> : null}
            {participants.length ? (
              <div role="list" aria-label={`In ${ch.name}`}>
                {participants.map((p) => (
                  <VoiceMember key={p.root_id} person={p} />
                ))}
              </div>
            ) : null}
          </div>
          </Fragment>
        );
      })}
      {reorder.lineAt(channels.length) ? <ChannelDropLine /> : null}
      <div role="status" aria-live="polite" className="sr-only" data-channel-order-status="voice">
        {reorder.announcement}
      </div>
    </>
  );
}

// One person in a voice channel: avatar, name, and mute and deafen flags.
function VoiceMember({ person }: { person: VoiceParticipant }) {
  return (
    <div role="listitem" className="flex h-[30px] shrink-0 items-center gap-s-2 pr-s-2 pl-9 text-sm font-medium text-muted-foreground">
      <UserAvatar name={person.display_name} seed={person.root_id} size="xs" className="size-[22px] text-[8px]" speaking={person.speaking} />
      <span className="min-w-0 flex-1 truncate" title={person.display_name}>{person.display_name}</span>
      {person.muted ? (
        <span className="inline-flex shrink-0 text-faint" title="Muted"><MicOff className="size-3.5" /><span className="sr-only">Muted</span></span>
      ) : null}
      {person.deafened ? (
        <span className="inline-flex shrink-0 text-danger-text" title="Deafened"><Headphones className="size-3.5" /><span className="sr-only">Deafened</span></span>
      ) : null}
    </div>
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
