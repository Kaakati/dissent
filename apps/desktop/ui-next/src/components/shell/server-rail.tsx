// The far-left column: Home (direct messages) on top, one round button per server, then "+" at the end of the list.
// The active place has a pill on its left edge (shell.css). Each button names itself in a tooltip to the right, as the
// current UI's data-tip labels do. Ported from the old UI's views/rail.js.
// The servers can be dragged into another order, and Alt+Shift+Up and Down move the focused one. The drag uses pointer
// events, not HTML5 drag and drop, which the webview does not deliver reliably. The order is this device's own and is
// saved with set_server_order; Home and "+" stay where they are.
import { has } from '@/lib/features';
import { Fragment, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent, type PointerEvent, type ReactElement } from 'react';
import { Plus } from 'lucide-react';
import { OwlMark } from '@/components/owl-mark';
import { GroupAvatar } from '@/components/shell/sidebar';
import { UserAvatar } from '@/components/user-avatar';
import { openServerDialog } from '@/components/dialogs/server-dialog';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { bannersStill, subscribeBannerMotion } from '@/lib/banners';
import { iconSrc } from '@/lib/images';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import { openServerRailMenu } from '@/components/menus/server-rail-menu';
import { useMutes } from '@/lib/mutes';
import { DEFAULT_PREFS, isServerMuted, loadServerPrefs, useServerPrefs } from '@/lib/server-prefs';
import { convKey, useShell } from '@/lib/store';
import type { ConversationSummary, Id } from '@/lib/types';
import { cn } from '@/lib/utils';

// A pointer must travel this far before it counts as a drag, so a click still opens the server.
const DRAG_START_PX = 5;

interface Drag {
  id: Id;
  // How far the pointer has moved from where the press began.
  dy: number;
  // The slot the server would drop into: before the server at this index, or the end of the list.
  dropAt: number;
}

interface Press {
  id: Id;
  pointerId: number;
  startY: number;
  dragging: boolean;
}

// A rail item's tooltip: its name, to the right of the icon, as in the reference client.
function Labelled({ label, children }: { label: string; children: ReactElement }) {
  return (
    <Tip label={label} side="right" sideOffset={16}>
      {children}
    </Tip>
  );
}

function DropLine() {
  return <div aria-hidden="true" className="h-0.5 w-8 shrink-0 rounded-sm bg-accent" />;
}

export function ServerRail() {
  const route = useShell((s) => s.route);
  const communities = useShell((s) => s.communities);
  const unread = useShell((s) => s.unread);
  const prefs = useServerPrefs((s) => s.prefs);
  // An animated icon shows its still frame while motion is held, so the rail redraws when that changes.
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  // The servers' preferences, for the muted servers' unread dots. Loaded again when the list of servers changes.
  const idKey = communities.map((c) => c.id).join(',');
  useEffect(() => {
    if (!has('server_prefs')) return;
    idKey.split(',').filter(Boolean).forEach((id) => void loadServerPrefs(id));
  }, [idKey]);
  const openHome = useShell((s) => s.openHome);
  const openServer = useShell((s) => s.openServer);
  const moveServer = useShell((s) => s.moveServer);

  const [drag, setDrag] = useState<Drag | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const rows = useRef(new Map<Id, HTMLDivElement>());
  const press = useRef<Press | null>(null);
  // Set when a drag ends, so the click that follows the release does not open the server.
  const justDragged = useRef(false);

  const onHome = route.kind === 'home';
  // Unread conversations (direct and group) make the Home badge; the number is the count of them.
  const dmUnread = [...unread].filter((k) => k.startsWith('dm:') || k.startsWith('group:')).length;
  const dragFrom = drag ? communities.findIndex((c) => c.id === drag.id) : -1;
  // A line right before or after the dragged server would not move it, so it is not shown.
  const showLineAt = (slot: number) => drag !== null && drag.dropAt === slot && slot !== dragFrom && slot !== dragFrom + 1;

  // The slot a pointer at `y` drops into: before the first server whose middle is below it, else the end.
  const slotAt = (y: number) => {
    for (let i = 0; i < communities.length; i++) {
      const el = rows.current.get(communities[i].id);
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      if (y < rect.top + rect.height / 2) return i;
    }
    return communities.length;
  };

  const place = (id: Id, to: number) => {
    const name = communities.find((c) => c.id === id)?.name ?? '';
    setAnnouncement(`Moved ${name} to position ${to + 1} of ${communities.length}`);
    void moveServer(id, to);
  };

  const onPointerDown = (e: PointerEvent<HTMLButtonElement>, id: Id) => {
    if (e.button !== 0) return;
    justDragged.current = false;
    press.current = { id, pointerId: e.pointerId, startY: e.clientY, dragging: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || p.pointerId !== e.pointerId) return;
    if (!p.dragging) {
      if (Math.abs(e.clientY - p.startY) < DRAG_START_PX) return;
      p.dragging = true;
    }
    setDrag({ id: p.id, dy: e.clientY - p.startY, dropAt: slotAt(e.clientY) });
  };

  const onPointerUp = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || p.pointerId !== e.pointerId) return;
    press.current = null;
    if (!p.dragging) return;
    // The click that follows this release is skipped. A drop over another server produces no click on this one, so the
    // flag clears after the release's own events instead of swallowing the next real click.
    justDragged.current = true;
    setTimeout(() => {
      justDragged.current = false;
    }, 0);
    setDrag(null);
    const from = communities.findIndex((c) => c.id === p.id);
    const dropAt = slotAt(e.clientY);
    const to = dropAt > from ? dropAt - 1 : dropAt;
    if (from >= 0 && to !== from) place(p.id, to);
  };

  const onPointerCancel = () => {
    press.current = null;
    setDrag(null);
  };

  // Alt+Shift+Up and Down move the focused server one place. Focus stays on it after the move.
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number, id: Id) => {
    if (!e.altKey || !e.shiftKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
    e.preventDefault();
    const to = index + (e.key === 'ArrowUp' ? -1 : 1);
    if (to < 0 || to >= communities.length) return;
    place(id, to);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-server-id="${id}"]`)?.focus());
  };

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
            className={cn('size-12 rounded-2xl bg-chat text-brand hover:bg-accent hover:text-white', onHome && 'rounded-2xl bg-accent text-white')}
          >
            <OwlMark className="size-6" />
          </Button>
        </Labelled>
        {dmUnread > 0 ? <Badge count={dmUnread} label={`${dmUnread} unread conversations`} /> : null}
      </div>

      <UnreadDms />

      <hr className="m-0 h-0.5 w-8 shrink-0 rounded-sm border-0 bg-chat" />

      {communities.map((c, index) => {
        const active = route.kind === 'server' && route.communityId === c.id;
        // A muted server shows no unread dot. Its mentions still count on the badge.
        const hasUnread = [...unread].some((k) => k.startsWith(`ch:${c.id}:`)) && !isServerMuted(prefs[c.id] ?? DEFAULT_PREFS);
        const image = iconSrc(c.id);
        const dragging = drag !== null && drag.id === c.id;
        return (
          <Fragment key={c.id}>
            {showLineAt(index) ? <DropLine /> : null}
            <div
              ref={(el) => {
                if (el) rows.current.set(c.id, el);
                else rows.current.delete(c.id);
              }}
              style={drag?.id === c.id ? { transform: `translateY(${drag.dy}px)` } : undefined}
              className={cn(
                'rail-item relative grid h-12 w-[72px] shrink-0 place-items-center',
                active && 'active',
                hasUnread && 'unread',
                dragging && 'z-[3] opacity-80',
              )}
            >
              <Labelled label={c.name}>
                <button
                  type="button"
                  role="tab"
                  aria-selected={active}
                  aria-current={active ? 'page' : undefined}
                  aria-label={c.name}
                  data-server-id={c.id}
                  onClick={() => {
                    if (justDragged.current) {
                      justDragged.current = false;
                      return;
                    }
                    void openServer(c.id);
                  }}
                  onPointerDown={(e) => onPointerDown(e, c.id)}
                  onPointerMove={onPointerMove}
                  onPointerUp={onPointerUp}
                  onContextMenu={(e) => void openServerRailMenu(e, c.id)}
                  onPointerCancel={onPointerCancel}
                  onKeyDown={(e) => onKeyDown(e, index, c.id)}
                  className={cn(
                    'relative grid size-12 touch-none place-items-center overflow-hidden rounded-2xl border-0 p-0 text-[15px] leading-none font-semibold text-white transition-[background-color] duration-150 motion-reduce:transition-none',
                    active && 'rounded-2xl',
                  )}
                  style={{ background: hueBackground(c.hue ?? hueOf(c.id)) }}
                >
                  {initials(c.name)}
                  {image ? <img src={image} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : null}
                </button>
              </Labelled>
              {has('mentions') && typeof c.mentions === 'number' && c.mentions > 0 ? <Badge count={c.mentions} label={`${c.mentions} mentions`} /> : null}
            </div>
          </Fragment>
        );
      })}
      {showLineAt(communities.length) ? <DropLine /> : null}

      <div className="rail-item relative grid h-12 w-[72px] shrink-0 place-items-center">
        <Labelled label="Add a Server">
          <Button
            variant="ghost"
            size="icon"
            aria-label="Create or join a server"
            onClick={() => openServerDialog()}
            className="size-12 rounded-2xl bg-chat text-accent-text hover:bg-accent hover:text-white disabled:opacity-100"
          >
            <Plus className="size-6" />
          </Button>
        </Labelled>
      </div>

      <div role="status" aria-live="polite" className="sr-only">
        {announcement}
      </div>
    </nav>
  );
}

// The unread direct messages: the avatars of the conversations with unread messages, newest first, between Home and the
// servers. Five show, and the rest become a "+N" that opens Home. A conversation that is muted does not show here.
const UNREAD_DMS_SHOWN = 5;
const LEAVE_MS = 200;

// The conversations on the rail. One that stops being unread keeps its avatar for a moment, so it can animate out.
function usePresentDms(items: ConversationSummary[]): { conv: ConversationSummary; leaving: boolean }[] {
  const [leaving, setLeaving] = useState<ConversationSummary[]>([]);
  const previous = useRef(items);
  useEffect(() => {
    const keep = new Set(items.map(convKey));
    const gone = previous.current.filter((c) => !keep.has(convKey(c)));
    previous.current = items;
    if (gone.length === 0) return;
    const goneKeys = new Set(gone.map(convKey));
    setLeaving((l) => [...l.filter((c) => !keep.has(convKey(c))), ...gone]);
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    setTimeout(() => setLeaving((l) => l.filter((c) => !goneKeys.has(convKey(c)))), reduced ? 0 : LEAVE_MS);
  }, [items]);
  const keep = new Set(items.map(convKey));
  return [
    ...items.map((conv) => ({ conv, leaving: false })),
    ...leaving.filter((c) => !keep.has(convKey(c))).map((conv) => ({ conv, leaving: true })),
  ];
}

function UnreadDms() {
  const conversations = useShell((s) => s.conversations);
  const mutes = useMutes((s) => s.mutes);
  const openConversation = useShell((s) => s.openConversation);
  const openHome = useShell((s) => s.openHome);
  const unread = useMemo(
    () => conversations.filter((c) => (c.unread ?? 0) > 0 && !(convKey(c) in mutes)),
    [conversations, mutes],
  );
  const shown = usePresentDms(unread);
  if (shown.length === 0) return null;
  const extra = unread.length - UNREAD_DMS_SHOWN;
  return (
    <div role="group" aria-label="Unread direct messages" className="flex shrink-0 flex-col items-center gap-[var(--rail-gap)]">
      {shown.slice(0, UNREAD_DMS_SHOWN).map(({ conv, leaving }) => {
        const name = conv.kind === 'group' ? conv.name : conv.display_name;
        const count = conv.unread ?? 0;
        return (
          <div
            key={convKey(conv)}
            className={cn('relative grid size-12 place-items-center', leaving ? 'rail-dm-leave' : 'rail-dm-enter')}
          >
            <Labelled label={name}>
              <button
                type="button"
                aria-label={`${name}, ${count} unread ${count === 1 ? 'message' : 'messages'}`}
                tabIndex={leaving ? -1 : undefined}
                onClick={() => void openConversation(conv)}
                className="grid size-12 place-items-center overflow-hidden rounded-full"
              >
                {conv.kind === 'group' ? (
                  <GroupAvatar conv={conv} large />
                ) : (
                  <UserAvatar name={name} seed={conv.peer} size="lg" />
                )}
              </button>
            </Labelled>
            {count > 0 ? <Badge count={count} label={`${count} unread`} /> : null}
          </div>
        );
      })}
      {extra > 0 ? (
        <Labelled label="More unread conversations">
          <button
            type="button"
            aria-label={`${extra} more unread ${extra === 1 ? 'conversation' : 'conversations'}`}
            onClick={() => void openHome()}
            className="grid size-12 place-items-center rounded-full bg-chat text-sm font-bold text-text-strong hover:bg-accent hover:text-white"
          >
            {`+${extra}`}
          </button>
        </Labelled>
      ) : null}
    </div>
  );
}

function Badge({ count, label }: { count: number; label: string }) {
  return (
    <span
      aria-label={label}
      className="absolute right-2 -bottom-0.5 z-[2] grid h-4 min-w-4 place-items-center rounded-full bg-accent px-1 text-tiny font-bold text-white shadow-[0_0_0_2px_var(--rail)] tabular-nums"
    >
      {count > 99 ? '99+' : count}
    </span>
  );
}
