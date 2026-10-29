// Moving channels in the sidebar, for the people who may manage them. A channel is dragged within its own kind: text among
// text, voice among voice. It works as the server rail's drag does (server-rail.tsx): pointer events rather than HTML5
// drag and drop, which the webview does not deliver reliably, a line where it will land, and Alt+Shift+Up and Down on the
// focused channel. The order is the server's, so the store publishes it for everyone and puts it back if that fails.
import { useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { useShell } from '@/lib/store';
import type { CommunityView, Id } from '@/lib/types';

// A pointer must travel this far before it counts as a drag, so a click still opens the channel.
const DRAG_START_PX = 5;

interface Drag {
  id: Id;
  dy: number;
  // The slot the channel would drop into: before the row at this index, or the end of the list.
  dropAt: number;
}

interface Press {
  id: Id;
  pointerId: number;
  startY: number;
  dragging: boolean;
}

// The full list of one kind with `id` moved to place `to` of the shown rows. Rows that are hidden (a muted channel under
// Hide Muted Channels) keep their place among the others.
export function moveInto(full: Id[], shown: Id[], id: Id, to: number): Id[] {
  const rest = shown.filter((x) => x !== id);
  const out = full.filter((x) => x !== id);
  const before = rest[to];
  if (before !== undefined) {
    out.splice(out.indexOf(before), 0, id);
  } else {
    const after = rest[rest.length - 1];
    out.splice(after === undefined ? out.length : out.indexOf(after) + 1, 0, id);
  }
  return out;
}

// Publishes the server's order with `id` moved to place `to` of the shown rows of its kind.
export function moveChannel(community: CommunityView, shown: Id[], id: Id, to: number): void {
  const voice = Boolean(community.channels.find((c) => c.id === id)?.voice);
  const ids = (v: boolean) => community.channels.filter((c) => Boolean(c.voice) === v).map((c) => c.id);
  const moved = moveInto(ids(voice), shown, id, to);
  void useShell.getState().reorderChannels(voice ? ids(false) : moved, voice ? moved : ids(true));
}

export function ChannelDropLine() {
  return <div aria-hidden="true" data-channel-drop-line className="mx-s-2 h-0.5 shrink-0 rounded-sm bg-accent" />;
}

// The drag and the keyboard moves of one list of rows. `shown` is the rows in order; `move` gets the place in it.
export function useChannelReorder(enabled: boolean, shown: { id: Id; name: string }[], move: (id: Id, to: number) => void) {
  const [drag, setDrag] = useState<Drag | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const rows = useRef(new Map<Id, HTMLElement>());
  const press = useRef<Press | null>(null);
  // Set when a drag ends, so the click that follows the release does not open the channel.
  const justDragged = useRef(false);

  const from = drag ? shown.findIndex((c) => c.id === drag.id) : -1;
  // A line right before or after the dragged row would not move it, so it is not shown.
  const lineAt = (slot: number) => drag !== null && drag.dropAt === slot && slot !== from && slot !== from + 1;

  const slotAt = (y: number) => {
    for (let i = 0; i < shown.length; i++) {
      const rect = rows.current.get(shown[i].id)?.getBoundingClientRect();
      if (rect && y < rect.top + rect.height / 2) return i;
    }
    return shown.length;
  };

  const place = (id: Id, to: number) => {
    const name = shown.find((c) => c.id === id)?.name ?? '';
    setAnnouncement(`Moved ${name} to position ${to + 1} of ${shown.length}`);
    move(id, to);
  };

  const rowRef = (id: Id) => (el: HTMLElement | null) => {
    if (el) rows.current.set(id, el);
    else rows.current.delete(id);
  };

  // Spread onto a row's button. Without permission it adds nothing, so the row cannot be dragged.
  const handlers = (id: Id, index: number) => {
    if (!enabled) return {};
    return {
      'data-channel-id': id,
      onPointerDown: (e: PointerEvent<HTMLButtonElement>) => {
        if (e.button !== 0) return;
        justDragged.current = false;
        press.current = { id, pointerId: e.pointerId, startY: e.clientY, dragging: false };
        e.currentTarget.setPointerCapture(e.pointerId);
      },
      onPointerMove: (e: PointerEvent<HTMLButtonElement>) => {
        const p = press.current;
        if (!p || p.pointerId !== e.pointerId) return;
        if (!p.dragging) {
          if (Math.abs(e.clientY - p.startY) < DRAG_START_PX) return;
          p.dragging = true;
        }
        setDrag({ id: p.id, dy: e.clientY - p.startY, dropAt: slotAt(e.clientY) });
      },
      onPointerUp: (e: PointerEvent<HTMLButtonElement>) => {
        const p = press.current;
        if (!p || p.pointerId !== e.pointerId) return;
        press.current = null;
        if (!p.dragging) return;
        justDragged.current = true;
        setDrag(null);
        const at = shown.findIndex((c) => c.id === p.id);
        const dropAt = slotAt(e.clientY);
        const to = dropAt > at ? dropAt - 1 : dropAt;
        if (at >= 0 && to !== at) place(p.id, to);
      },
      onPointerCancel: () => {
        press.current = null;
        setDrag(null);
      },
      // Alt+Shift+Up and Down move the focused channel one place. Focus stays on it after the move.
      onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => {
        if (!e.altKey || !e.shiftKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
        e.preventDefault();
        const to = index + (e.key === 'ArrowUp' ? -1 : 1);
        if (to < 0 || to >= shown.length) return;
        place(id, to);
        requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-channel-id="${id}"]`)?.focus());
      },
    };
  };

  // Swallows the click that ends a drag. Returns true when the click should open the channel.
  const clickOpens = () => {
    if (!justDragged.current) return true;
    justDragged.current = false;
    return false;
  };

  const rowStyle = (id: Id) => (drag?.id === id ? { transform: `translateY(${drag.dy}px)` } : undefined);
  const dragging = (id: Id) => drag?.id === id;

  return { announcement, lineAt, rowRef, handlers, clickOpens, rowStyle, dragging, place };
}
