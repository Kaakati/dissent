// The channel list of a server: channels outside any category, and categories of text and voice channels, in the
// server's layout (lib/channel-layout.ts). A category header opens and closes its channels, and each person's open or
// closed state stays on their device. People who may arrange channels drag a channel into, out of and within a
// category, drag a category by its header, and do the same from the keyboard with Alt+Shift+Up and Down or from the
// menus. The drag uses pointer events, as the server rail's does, with a line or a highlighted header where it lands.
import { Fragment, useRef, useState, type ButtonHTMLAttributes, type KeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent } from 'react';
import { ArrowDown, ArrowUp, ChevronDown, FolderInput, FolderPlus, Hash, Headphones, Lock, MicOff, Pencil, Plus, Settings, Trash2, UserPlus, Volume2 } from 'lucide-react';
import { openChannelSettings } from '@/components/channel-settings/channel-settings';
import { deleteCategoryAsked, openCreateCategoryDialog, openRenameCategoryDialog } from '@/components/dialogs/category-dialogs';
import { openCreateChannelDialog } from '@/components/dialogs/create-channel-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { UserAvatar } from '@/components/user-avatar';
import { openVoiceChannel } from '@/lib/calls';
import {
  VIRTUAL_TEXT,
  VIRTUAL_VOICE,
  categoriesOf,
  layoutOf,
  moveCategory,
  moveChannel,
  placeOf,
  stepCategory,
  stepChannel,
  topKey,
  withoutChannel,
} from '@/lib/channel-layout';
import { has } from '@/lib/features';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import { useMutes } from '@/lib/mutes';
import { DEFAULT_PREFS, useServerPrefs } from '@/lib/server-prefs';
import { channelKey, useShell } from '@/lib/store';
import type { ChannelView, CommunityView, Id, LayoutEntry, VoiceParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';
import { rowClass, UnreadDot } from './sidebar-rows';

// A pointer must travel this far before it counts as a drag, so a click still opens the channel.
const DRAG_START_PX = 5;
// The top part of a category header that drops above the category rather than into it.
const HEADER_ABOVE = 0.3;
const KEYS = 'Alt+Shift+ArrowUp Alt+Shift+ArrowDown';

type Target = { into: string } | { beforeRow: Id } | { beforeTop: number } | { end: true };
type Grab = { channel: Id } | { category: string };

interface Drag {
  grab: Grab;
  dy: number;
  target: Target | null;
}

interface Press {
  grab: Grab;
  pointerId: number;
  startY: number;
  dragging: boolean;
}

// What the pointer can land on, in the order the sidebar shows it.
type Item = { header: string; top: number } | { row: Id; top: number };

type Category = Extract<LayoutEntry, { kind: 'category' }>;

const keyOfCat = (c: Category) => c.id ?? '';

// ---- the open or closed state of each category, kept on this device ----

const collapsedKey = (communityId: Id) => `dissent-category-collapsed:${communityId}`;

function readCollapsed(communityId: Id): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(collapsedKey(communityId));
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

function useCollapsed(communityId: Id) {
  const [state, setState] = useState(() => ({ id: communityId, map: readCollapsed(communityId) }));
  // Another server's state is read when the server changes.
  const current = state.id === communityId ? state.map : readCollapsed(communityId);
  if (state.id !== communityId) setState({ id: communityId, map: current });
  const isCollapsed = (c: Category) => current[keyOfCat(c)] ?? Boolean(c.collapsed);
  const toggle = (c: Category) => {
    const map = { ...current, [keyOfCat(c)]: !isCollapsed(c) };
    setState({ id: communityId, map });
    try {
      localStorage.setItem(collapsedKey(communityId), JSON.stringify(map));
    } catch {
      // Without storage the state lasts until the app closes.
    }
  };
  return { isCollapsed, toggle };
}

// ---- moves ----

// The layout with the dragged channel or category placed at `target`.
// Indices are taken after the dragged entry leaves, so each place is found again in the layout without it.
function placed(entries: LayoutEntry[], grab: Grab, target: Target): LayoutEntry[] {
  const anchor = 'beforeTop' in target && entries[target.beforeTop] ? topKey(entries[target.beforeTop]) : null;
  if ('category' in grab) {
    const rest = entries.filter((e) => !(e.kind === 'category' && e.id === grab.category));
    const index = anchor === null ? -1 : rest.findIndex((e) => topKey(e) === anchor);
    return moveCategory(entries, grab.category, index < 0 ? rest.length : index);
  }
  const id = grab.channel;
  if ('into' in target) return moveChannel(entries, id, target.into, 0);
  const rest = withoutChannel(entries, id);
  if ('beforeRow' in target) {
    const at = placeOf(rest, target.beforeRow);
    return at ? moveChannel(entries, id, at.category, at.index) : entries;
  }
  const index = anchor === null ? -1 : rest.findIndex((e) => topKey(e) === anchor);
  return moveChannel(entries, id, null, index < 0 ? rest.length : index);
}

const sameTarget = (a: Target | null, b: Target | null) => JSON.stringify(a) === JSON.stringify(b);

// ---- the tree ----

export function ChannelTree({ community, canManage }: { community: CommunityView; canManage: boolean }) {
  const channelId = useShell((s) => s.channelId);
  const voiceChannelId = useShell((s) => s.voiceChannelId);
  const connection = useShell((s) => s.voiceConnection);
  const page = useShell((s) => s.page);
  const setLayout = useShell((s) => s.setLayout);
  const prefs = useServerPrefs((s) => s.prefs[community.id]) ?? DEFAULT_PREFS;
  const mutes = useMutes((s) => s.mutes);
  const { isCollapsed, toggle } = useCollapsed(community.id);
  const editable = canManage && (community.can_manage_channels ?? true) && has('channel_categories');

  const entries = layoutOf(community);
  const byId = new Map(community.channels.map((c) => [c.id, c]));
  // Hide Muted Channels hides a muted text channel, except the one open now, so the user keeps their place.
  const hidden = (ch: ChannelView) =>
    !ch.voice && prefs.hide_muted_channels && channelKey(community.id, ch.id) in mutes && ch.id !== channelId;
  const isOpen = (ch: ChannelView) => (ch.voice ? voiceChannelId === ch.id || connection?.channelId === ch.id : ch.id === channelId && !voiceChannelId && !page);

  const [drag, setDrag] = useState<Drag | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const rows = useRef(new Map<string, HTMLElement>());
  const press = useRef<Press | null>(null);
  const justDragged = useRef(false);

  // The rows and headers the pointer and the keys can reach, top to bottom.
  const items: Item[] = [];
  entries.forEach((e, top) => {
    if (e.kind === 'channel') {
      const ch = byId.get(e.id);
      if (ch && !hidden(ch)) items.push({ row: e.id, top });
      return;
    }
    items.push({ header: keyOfCat(e), top });
    const closed = isCollapsed(e);
    for (const id of e.channels) {
      const ch = byId.get(id);
      if (ch && !hidden(ch) && (!closed || isOpen(ch))) items.push({ row: id, top });
    }
  });

  const refKey = (item: Item) => ('row' in item ? `row:${item.row}` : `header:${item.header}`);
  const setRef = (key: string) => (el: HTMLElement | null) => {
    if (el) rows.current.set(key, el);
    else rows.current.delete(key);
  };

  // The dragged row or category moves with the pointer, so it is never its own target.
  const targetAt = (grab: Grab, y: number): Target => {
    if ('category' in grab) {
      // A category lands only between top-level entries.
      for (let top = 0; top < entries.length; top++) {
        const e = entries[top];
        if (e.kind === 'category' && keyOfCat(e) === grab.category) continue;
        const key = e.kind === 'channel' ? `top:${e.id}` : `top:${keyOfCat(e)}`;
        const rect = rows.current.get(key)?.getBoundingClientRect();
        if (rect && y < rect.top + rect.height / 2) return { beforeTop: top };
      }
      return { end: true };
    }
    for (const item of items) {
      if ('row' in item && item.row === grab.channel) continue;
      const rect = rows.current.get(refKey(item))?.getBoundingClientRect();
      if (!rect) continue;
      if ('header' in item && y >= rect.top && y < rect.bottom) {
        return y < rect.top + rect.height * HEADER_ABOVE ? { beforeTop: item.top } : { into: item.header };
      }
      if (y < rect.top + rect.height / 2) return 'row' in item ? { beforeRow: item.row } : { beforeTop: item.top };
    }
    return { end: true };
  };

  const nameOf = (grab: Grab) =>
    'channel' in grab ? (byId.get(grab.channel)?.name ?? '') : (categoriesOf(entries).find((c) => keyOfCat(c) === grab.category)?.name ?? '');

  // Says where a move left the channel or the category, for screen readers.
  const announce = (grab: Grab, next: LayoutEntry[]) => {
    if ('category' in grab) {
      const top = next.findIndex((e) => e.kind === 'category' && keyOfCat(e) === grab.category);
      setAnnouncement(`Moved ${nameOf(grab)} to position ${top + 1} of ${next.length}`);
      return;
    }
    const at = placeOf(next, grab.channel);
    if (!at) return;
    const cat = at.category === null ? null : categoriesOf(next).find((c) => keyOfCat(c) === at.category);
    setAnnouncement(cat ? `Moved ${nameOf(grab)} to position ${at.index + 1} in ${cat.name}` : `Moved ${nameOf(grab)} outside categories`);
  };

  const apply = (grab: Grab, next: LayoutEntry[] | null) => {
    if (!next || JSON.stringify(next) === JSON.stringify(entries)) return;
    announce(grab, next);
    void setLayout(next, 'category' in grab ? 'Could not move the category' : 'Could not move the channel');
  };

  const refocus = (grab: Grab) =>
    requestAnimationFrame(() => {
      const sel = 'channel' in grab ? `[data-channel-id="${grab.channel}"]` : `[data-category-key="${grab.category}"]`;
      document.querySelector<HTMLElement>(sel)?.focus();
    });

  // Spread onto a row's or a header's button. Without permission it adds nothing, so nothing drags.
  const handlers = (grab: Grab): ButtonHTMLAttributes<HTMLButtonElement> => {
    if (!editable) return {};
    return {
      onPointerDown: (e: PointerEvent<HTMLButtonElement>) => {
        if (e.button !== 0) return;
        justDragged.current = false;
        press.current = { grab, pointerId: e.pointerId, startY: e.clientY, dragging: false };
        e.currentTarget.setPointerCapture(e.pointerId);
      },
      onPointerMove: (e: PointerEvent<HTMLButtonElement>) => {
        const p = press.current;
        if (!p || p.pointerId !== e.pointerId) return;
        if (!p.dragging) {
          if (Math.abs(e.clientY - p.startY) < DRAG_START_PX) return;
          p.dragging = true;
        }
        setDrag({ grab: p.grab, dy: e.clientY - p.startY, target: targetAt(p.grab, e.clientY) });
      },
      onPointerUp: (e: PointerEvent<HTMLButtonElement>) => {
        const p = press.current;
        if (!p || p.pointerId !== e.pointerId) return;
        press.current = null;
        if (!p.dragging) return;
        justDragged.current = true;
        setDrag(null);
        apply(p.grab, placed(entries, p.grab, targetAt(p.grab, e.clientY)));
      },
      onPointerCancel: () => {
        press.current = null;
        setDrag(null);
      },
      onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => {
        if (!e.altKey || !e.shiftKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
        e.preventDefault();
        const dir = e.key === 'ArrowUp' ? -1 : 1;
        apply(grab, 'channel' in grab ? stepChannel(entries, grab.channel, dir) : stepCategory(entries, grab.category, dir));
        refocus(grab);
      },
      'aria-keyshortcuts': KEYS,
    };
  };

  // Swallows the click that ends a drag.
  const clickGoes = () => {
    if (!justDragged.current) return true;
    justDragged.current = false;
    return false;
  };

  const lineBefore = (target: Target) => drag !== null && sameTarget(drag.target, target);
  const dragged = (grab: Grab) => drag !== null && JSON.stringify(drag.grab) === JSON.stringify(grab);
  const lift = (grab: Grab) => (dragged(grab) && drag ? { transform: `translateY(${drag.dy}px)` } : undefined);

  const moveMenu = (id: Id): MenuEntry[] => {
    if (!editable) return [];
    const up = stepChannel(entries, id, -1);
    const down = stepChannel(entries, id, 1);
    const at = placeOf(entries, id);
    const cats = categoriesOf(entries);
    return [
      'sep',
      { label: 'Move Up', icon: ArrowUp, disabled: !up, onPick: () => apply({ channel: id }, up) },
      { label: 'Move Down', icon: ArrowDown, disabled: !down, onPick: () => apply({ channel: id }, down) },
      {
        label: 'Move to Category',
        icon: FolderInput,
        submenu: [
          ...cats.map((c) => ({
            label: c.name,
            radio: true,
            checked: at?.category === keyOfCat(c),
            onPick: () => apply({ channel: id }, moveChannel(entries, id, keyOfCat(c), c.channels.filter((x) => x !== id).length)),
          })),
          { label: 'No Category', radio: true, checked: at?.category === null, onPick: () => apply({ channel: id }, moveChannel(entries, id, null, entries.length)) },
        ],
      },
    ];
  };

  const channelMenu = (e: ReactMouseEvent<HTMLElement>, id: Id) => {
    e.preventDefault();
    const r = e.currentTarget.getBoundingClientRect();
    openContextMenu(e.clientX || r.left + 20, e.clientY || r.bottom, [
      { label: 'Create invite', icon: UserPlus, onPick: () => openInviteDialog(community.id) },
      { label: 'Edit Channel', icon: Settings, onPick: () => openChannelSettings(community.id, id) },
      ...moveMenu(id),
    ]);
  };

  const createIn = (c: Category) => {
    const key = keyOfCat(c);
    if (key === VIRTUAL_VOICE) openCreateChannelDialog(community.id, 'voice');
    else if (key === VIRTUAL_TEXT) openCreateChannelDialog(community.id);
    else openCreateChannelDialog(community.id, 'text', key);
  };

  const headerMenu = (e: ReactMouseEvent<HTMLElement>, c: Category) => {
    e.preventDefault();
    const r = e.currentTarget.getBoundingClientRect();
    const key = keyOfCat(c);
    const up = stepCategory(entries, key, -1);
    const down = stepCategory(entries, key, 1);
    openContextMenu(e.clientX || r.left + 20, e.clientY || r.bottom, [
      { label: 'Create Channel', icon: Plus, onPick: () => createIn(c) },
      { label: 'Create Category', icon: FolderPlus, onPick: () => openCreateCategoryDialog() },
      'sep',
      { label: 'Rename Category', icon: Pencil, onPick: () => openRenameCategoryDialog(key, c.name) },
      { label: 'Move Up', icon: ArrowUp, disabled: !up, onPick: () => apply({ category: key }, up) },
      { label: 'Move Down', icon: ArrowDown, disabled: !down, onPick: () => apply({ category: key }, down) },
      'sep',
      { label: 'Delete Category', icon: Trash2, danger: true, onPick: () => void deleteCategoryAsked(key, c.name) },
    ]);
  };

  const row = (id: Id) => {
    const ch = byId.get(id);
    if (!ch || hidden(ch)) return null;
    const grab = { channel: id };
    const props: RowProps = {
      community,
      ch,
      canManage,
      lifted: dragged(grab),
      style: lift(grab),
      rowRef: setRef(`row:${id}`),
      handlers: editable ? { ...handlers(grab), ...{ 'data-channel-id': id } } : {},
      clickGoes,
      onMenu: canManage ? (e) => channelMenu(e, id) : undefined,
    };
    return (
      <Fragment key={id}>
        {lineBefore({ beforeRow: id }) ? <DropLine /> : null}
        {ch.voice ? <VoiceRow {...props} /> : <TextRow {...props} />}
      </Fragment>
    );
  };

  return (
    <>
      {entries.map((e, top) => {
        if (e.kind === 'channel') {
          const shown = row(e.id);
          if (!shown) return null;
          return (
            <Fragment key={`top:${e.id}`}>
              {lineBefore({ beforeTop: top }) ? <DropLine /> : null}
              <div ref={setRef(`top:${e.id}`)} data-top-channel={e.id} className="flex shrink-0 flex-col">{shown}</div>
            </Fragment>
          );
        }
        const key = keyOfCat(e);
        const closed = isCollapsed(e);
        const grab = { category: key };
        const into = drag !== null && sameTarget(drag.target, { into: key });
        const virtualPlus = key === VIRTUAL_TEXT ? ['Create channel', 'Create Channel'] : key === VIRTUAL_VOICE ? ['Create voice channel', 'Create Voice Channel'] : null;
        return (
          <Fragment key={`top:${key}`}>
            {lineBefore({ beforeTop: top }) ? <DropLine /> : null}
            <div
              ref={setRef(`top:${key}`)}
              style={lift(grab)}
              className={cn('flex shrink-0 flex-col gap-0.5', top > 0 && 'mt-s-2', dragged(grab) && 'z-[3] opacity-80')}
              role="group"
              aria-label={e.name}
              data-category={key}
            >
              <div
                ref={setRef(`header:${key}`)}
                data-drop-into={into ? '' : undefined}
                className={cn('flex h-8 shrink-0 items-center justify-between rounded-md text-muted-foreground', into && 'bg-accent-tint ring-1 ring-accent')}
              >
                <button
                  type="button"
                  aria-expanded={!closed}
                  data-category-key={key}
                  className={cn('flex h-8 min-w-0 flex-1 items-center gap-1 pl-0.5 text-left text-meta font-semibold tracking-[.02em] uppercase hover:text-text', editable && 'touch-none')}
                  onClick={() => {
                    if (clickGoes()) toggle(e);
                  }}
                  onContextMenu={editable ? (ev) => headerMenu(ev, e) : undefined}
                  {...handlers(grab)}
                >
                  <ChevronDown className={cn('size-3 shrink-0 transition-transform motion-reduce:transition-none', closed && '-rotate-90')} aria-hidden="true" />
                  <span className="truncate">{e.name}</span>
                </button>
                {canManage ? (
                  <Tip label={virtualPlus ? virtualPlus[1] : 'Create Channel'}>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={virtualPlus ? virtualPlus[0] : `Create channel in ${e.name}`}
                      onClick={() => createIn(e)}
                    >
                      <Plus className="size-4" />
                    </Button>
                  </Tip>
                ) : null}
              </div>
              {e.channels.map((id) => {
                const ch = byId.get(id);
                if (!ch || (closed && !isOpen(ch))) return null;
                return row(id);
              })}
            </div>
          </Fragment>
        );
      })}
      {drag !== null && sameTarget(drag.target, { end: true }) ? <DropLine /> : null}
      {community.channels.length === 0 ? <p className="p-s-2 text-sm text-muted-foreground">No channels yet.</p> : null}
      <div role="status" aria-live="polite" className="sr-only" data-channel-order-status>
        {announcement}
      </div>
    </>
  );
}

function DropLine() {
  return <div aria-hidden="true" data-channel-drop-line className="mx-s-2 h-0.5 shrink-0 rounded-sm bg-accent" />;
}

// ---- rows ----

interface RowProps {
  community: CommunityView;
  ch: ChannelView;
  canManage: boolean;
  lifted: boolean;
  style: { transform: string } | undefined;
  rowRef: (el: HTMLElement | null) => void;
  handlers: ButtonHTMLAttributes<HTMLButtonElement>;
  clickGoes: () => boolean;
  onMenu: ((e: ReactMouseEvent<HTMLElement>) => void) | undefined;
}

function TextRow({ community, ch, canManage, lifted, style, rowRef, handlers, clickGoes, onMenu }: RowProps) {
  const channelId = useShell((s) => s.channelId);
  const page = useShell((s) => s.page);
  const voiceChannelId = useShell((s) => s.voiceChannelId);
  const unread = useShell((s) => s.unread);
  const openChannel = useShell((s) => s.openChannel);
  const active = ch.id === channelId && !voiceChannelId && !page;
  const isUnread = unread.has(`ch:${community.id}:${ch.id}`) && !active;
  return (
    <div ref={rowRef} style={style} className={cn('group relative flex shrink-0 flex-col', lifted && 'z-[3] opacity-80')}>
      <button
        type="button"
        className={cn(rowClass(active, isUnread), handlers.onPointerDown ? 'touch-none' : null)}
        aria-current={active ? 'true' : undefined}
        onClick={() => {
          if (clickGoes()) void openChannel(ch.id);
        }}
        onContextMenu={onMenu}
        {...handlers}
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
  );
}

function VoiceRow({ community, ch, canManage, lifted, style, rowRef, handlers, clickGoes, onMenu }: RowProps) {
  const voiceChannelId = useShell((s) => s.voiceChannelId);
  const connection = useShell((s) => s.voiceConnection);
  const me = useShell((s) => s.me);
  const call = useShell((s) => s.call);
  const active = voiceChannelId === ch.id;
  const connected = connection?.channelId === ch.id;
  const others: VoiceParticipant[] = (community.voice_channels ?? []).find((v) => v.id === ch.id)?.participants ?? [];
  // The engine lists everyone but this device; the channel you are connected to lists you first.
  const participants: VoiceParticipant[] = connected && me && call
    ? [{ root_id: me.root_id, display_name: me.display_name, muted: call.muted, deafened: call.deafened, speaking: false }, ...others]
    : others;
  return (
    <div ref={rowRef} style={style} className={cn('group relative flex flex-col gap-0.5', lifted && 'z-[3] opacity-80')}>
      <button
        type="button"
        className={cn(rowClass(active, false), 'font-medium', handlers.onPointerDown ? 'touch-none' : null)}
        aria-current={active ? 'true' : undefined}
        title={connected ? `Connected to ${ch.name}` : `Join ${ch.name}`}
        onClick={() => {
          if (clickGoes()) void openVoiceChannel(ch.id);
        }}
        onContextMenu={onMenu}
        {...handlers}
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

