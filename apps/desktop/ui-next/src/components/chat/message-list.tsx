// The message list: date dividers, the "New messages" line, grouping of consecutive messages by author and time, the
// notices around the history, the search filter, scroll anchoring, and the keys for moving between messages. Ported from
// the old UI's views/chat.js (render, rowsFor, onListKey).
import { useEffect, useLayoutEffect, useRef, type KeyboardEvent, type MutableRefObject, type ReactNode } from 'react';
import { Hash, Loader2, MessageCircle, Search } from 'lucide-react';
import { has } from '@/lib/features';
import { catchUpOpen, loadOlder, useChat } from '@/lib/chat';
import { dayKey, dayLabel } from '@/lib/time';
import { useShell } from '@/lib/store';
import { setAtLatest } from '@/lib/viewing';
import { EmptyNode, SkeletonRows } from './parts';
import { AgeGate, EmptyPane, KeyChangeBanner, Welcome } from './chat-notices';
import { MessageRow, SystemRow } from './message-row';
import { openMessageMenu } from './message-menu';
import type { ChatView, ViewItem } from './use-chat-view';

const GROUP_MS = 7 * 60 * 1000; // consecutive messages by one author within this gap share a header
const NEAR_BOTTOM_PX = 80; // within this distance of the end, a new message keeps the list at the end
const NEAR_TOP_PX = 240; // within this distance of the start, an older page of history is asked for

// Items that match the search text in their body or their author's name.
function filtered(items: ViewItem[], filter: string): ViewItem[] {
  const q = filter.trim().toLowerCase();
  if (!q) return items;
  return items.filter((item) => (item.text ?? '').toLowerCase().includes(q) || (item.authorName ?? '').toLowerCase().includes(q));
}

// Adds date dividers and groups consecutive messages from one author into a single header. The call chat uses it too, so
// a message reads the same there.
export function rowsFor(items: ViewItem[], view: ChatView, newFromId: string | null): ReactNode[] {
  const communityId = view.target.kind === 'channel' ? view.target.communityId : null;
  const out: ReactNode[] = [];
  let prev: ViewItem | null = null;
  let afterSystem = false; // a membership line starts a new header for the next message, even from the same author
  for (const item of items) {
    const newDay = !prev || dayKey(prev.ms) !== dayKey(item.ms);
    if (newDay) {
      out.push(
        <div key={`day-${item.id}`} className="date-divider" role="separator">
          <span>{dayLabel(item.ms)}</span>
        </div>,
      );
    }
    if (item.system) {
      out.push(<SystemRow key={item.id} item={item} />);
      afterSystem = true;
      prev = item;
      continue;
    }
    const start: boolean = !prev || newDay || afterSystem || prev.author !== item.author || item.ms - prev.ms > GROUP_MS || Boolean(item.replyTo);
    if (item.id === newFromId) {
      out.push(
        <div key={`new-${item.id}`} className="new-divider" role="separator">
          <span>New messages</span>
        </div>,
      );
    }
    out.push(
      <MessageRow key={item.id} item={item} view={view} start={start || item.id === newFromId} communityId={communityId} />,
    );
    afterSystem = false;
    prev = item;
  }
  return out;
}

// Keys in the list: the Menu key and Shift+F10 open a focused message's menu, Up and Down move between messages, and R
// replies to the focused one.
export function listKey(e: KeyboardEvent<HTMLDivElement>, list: HTMLDivElement | null, items: ViewItem[], view: ChatView | null): void {
  const focused = (e.target as HTMLElement).closest('.msg') as HTMLElement | null;
  const item = focused ? items.find((m) => m.id === focused.dataset.id) : undefined;
  if ((e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) && focused && e.target === focused) {
    e.preventDefault();
    if (item && view) {
      const r = focused.getBoundingClientRect();
      openMessageMenu(r.left + 40, r.top + 16, item, view, focused);
    }
    return;
  }
  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
    if (e.target !== list && !(focused && e.target === focused)) return;
    const rows = list ? [...list.querySelectorAll<HTMLElement>('.msg')] : [];
    if (!rows.length) return;
    const i = focused ? rows.indexOf(focused) : -1;
    const last = rows.length - 1;
    const next = e.key === 'ArrowDown' ? (i < 0 ? 0 : Math.min(last, i + 1)) : (i < 0 ? last : Math.max(0, i - 1));
    e.preventDefault();
    rows[next].focus();
    return;
  }
  if (e.key.toLowerCase() === 'r' && !e.ctrlKey && !e.metaKey && !e.altKey && focused && e.target === focused) {
    if (view?.social && item && !item.deleted) {
      e.preventDefault();
      useChat.getState().startReply(item.id, item.authorName || 'unknown');
    }
  }
}

// The message at the top of the list when a page of older history is asked for, and its distance from the list's top.
type Anchor = { id: string; top: number };


// Asks for an older page while the list is at its top. The message that was at the top is recorded, and the layout effect
// holds it in place once the page has rendered, so the page comes in above it without the list jumping.
async function loadKeepingPlace(el: HTMLElement, anchor: MutableRefObject<Anchor | null>): Promise<void> {
  const top = el.getBoundingClientRect().top;
  const row = [...el.querySelectorAll<HTMLElement>('.msg')].find((n) => n.getBoundingClientRect().top >= top);
  anchor.current = row?.dataset.id ? { id: row.dataset.id, top: row.getBoundingClientRect().top } : null;
  if (!(await loadOlder())) {
    anchor.current = null;
    return;
  }
  // The layout effect holds the anchor at the commit that renders the page, and clears it. A page that never renders
  // does not keep the anchor for long.
  const held = anchor.current;
  setTimeout(() => {
    if (anchor.current === held) anchor.current = null;
  }, 2000);
}

export function MessageList({ view, gated }: { view: ChatView | null; gated: boolean }) {
  const loading = useChat((s) => s.loading);
  const historyStart = useChat((s) => s.historyStart);
  const loadingOlder = useChat((s) => s.loadingOlder);
  const filter = useChat((s) => s.filter);
  const newFromId = useChat((s) => s.newFromId);
  const peerProfile = useChat((s) => s.peerProfile);
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  const list = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const anchor = useRef<Anchor | null>(null);
  const lastKey = useRef<string | null>(null);

  // A conversation opened starts at its end; after that the list stays at the end while it was there, so a new message
  // comes into view and a list scrolled up for older messages keeps its place.
  const key = view?.target.key ?? null;
  useLayoutEffect(() => {
    const el = list.current;
    if (!el) return;
    // A page has come in above the message that was at the top: hold that message where it was. Waits for the load to end,
    // so the loader row that shows meanwhile does not count as the page.
    const held = anchor.current;
    const row = held ? [...el.querySelectorAll<HTMLElement>('.msg')].find((n) => n.dataset.id === held.id) : undefined;
    if (held && row && !useChat.getState().loadingOlder) el.scrollTop += row.getBoundingClientRect().top - held.top;
    const switched = lastKey.current !== key;
    lastKey.current = key;
    if (switched) atBottom.current = true;
    if (switched || atBottom.current) el.scrollTop = el.scrollHeight;
    setAtLatest(atBottom.current);
  });

  // Content that grows after it renders (an image, an invite card, a preview) changes the list's height without a render.
  // A list at its end follows it down, and a page of older history being held stays where it was. Each child is watched,
  // since the list's own size does not change.
  useEffect(() => {
    const el = list.current;
    if (!el) return;
    const watched = new Set<Element>();
    const follow = new ResizeObserver(() => {
      const held = anchor.current;
      const row = held ? [...el.querySelectorAll<HTMLElement>('.msg')].find((n) => n.dataset.id === held.id) : undefined;
      if (held && row) el.scrollTop += row.getBoundingClientRect().top - held.top;
      else if (atBottom.current) el.scrollTop = el.scrollHeight;
    });
    const watch = () => {
      for (const child of el.children) {
        if (!watched.has(child)) {
          watched.add(child);
          follow.observe(child);
        }
      }
    };
    watch();
    const children = new MutationObserver(watch);
    children.observe(el, { childList: true });
    return () => {
      children.disconnect();
      follow.disconnect();
    };
  }, []);

  const items = view?.items ?? [];
  const shown = view ? filtered(items, filter) : [];
  let content: ReactNode;
  if (!view) {
    content = route.kind === 'server' && !community ? <SkeletonRows count={6} /> : <EmptyPane />;
  } else if (gated) {
    content = <AgeGate view={view} />;
  } else {
    let rows: ReactNode;
    if (loading && items.length === 0) rows = <SkeletonRows count={6} />;
    else if (items.length === 0) rows = <EmptyNode icon={view.dm ? MessageCircle : Hash} title="No messages yet" text="Say hello." />;
    else if (shown.length === 0) rows = <EmptyNode icon={Search} title="No matches" text={`No messages match "${filter}".`} />;
    else rows = rowsFor(shown, view, newFromId);
    const keyChange = view.dm && !view.group && has('key_change') && peerProfile?.key_changed_ms ? peerProfile.key_changed_ms : null;
    content = (
      <>
        {loadingOlder ? (
          <div role="status" className="flex items-center justify-center gap-s-2 py-s-2 text-meta text-muted-foreground">
            <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            Loading older messages
          </div>
        ) : null}
        {historyStart && !filter ? <Welcome view={view} peerProfile={peerProfile} /> : null}
        {keyChange ? <KeyChangeBanner view={view} keyChangedMs={keyChange} /> : null}
        {rows}
      </>
    );
  }

  return (
    <div
      ref={list}
      className="messages flex-1"
      role="log"
      aria-label="Messages"
      aria-live="polite"
      aria-busy={loading && items.length === 0 ? 'true' : 'false'}
      tabIndex={0}
      // Input by the user ends the hold on a page of older history, so the list is never pulled back from a scroll.
      onWheel={() => {
        anchor.current = null;
      }}
      onTouchStart={() => {
        anchor.current = null;
      }}
      onScroll={(e) => {
        const el = e.currentTarget;
        const was = atBottom.current;
        atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
        setAtLatest(atBottom.current);
        // Back at the end: what arrived while the list was scrolled up is read now.
        if (atBottom.current && !was) catchUpOpen();
        if (el.scrollTop < NEAR_TOP_PX) void loadKeepingPlace(el, anchor);
      }}
      onKeyDown={(e) => listKey(e, list.current, items, view)}
    >
      {content}
    </div>
  );
}
