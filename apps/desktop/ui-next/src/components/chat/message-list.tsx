// The message list: date dividers, the "New messages" line, grouping of consecutive messages by author and time, the
// notices around the history, the search filter, scroll anchoring, and the keys for moving between messages. Ported from
// ui/views/chat.js (render, rowsFor, onListKey).
import { useLayoutEffect, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { Hash, MessageCircle, Search } from 'lucide-react';
import { has } from '@/lib/features';
import { useChat } from '@/lib/chat';
import { dayKey, dayLabel } from '@/lib/time';
import { useShell } from '@/lib/store';
import { EmptyNode, SkeletonRows } from './parts';
import { AgeGate, EmptyPane, KeyChangeBanner, Welcome } from './chat-notices';
import { MessageRow, SystemRow } from './message-row';
import { openMessageMenu } from './message-menu';
import type { ChatView, ViewItem } from './use-chat-view';

const GROUP_MS = 7 * 60 * 1000; // consecutive messages by one author within this gap share a header
const NEAR_BOTTOM_PX = 80; // within this distance of the end, a new message keeps the list at the end

// Items that match the search text in their body or their author's name.
function filtered(items: ViewItem[], filter: string): ViewItem[] {
  const q = filter.trim().toLowerCase();
  if (!q) return items;
  return items.filter((item) => (item.text ?? '').toLowerCase().includes(q) || (item.authorName ?? '').toLowerCase().includes(q));
}

// Adds date dividers and groups consecutive messages from one author into a single header.
function rowsFor(items: ViewItem[], view: ChatView, newFromId: string | null): ReactNode[] {
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
function listKey(e: KeyboardEvent<HTMLDivElement>, list: HTMLDivElement | null, items: ViewItem[], view: ChatView | null): void {
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

export function MessageList({ view, gated }: { view: ChatView | null; gated: boolean }) {
  const loading = useChat((s) => s.loading);
  const historyStart = useChat((s) => s.historyStart);
  const filter = useChat((s) => s.filter);
  const newFromId = useChat((s) => s.newFromId);
  const peerProfile = useChat((s) => s.peerProfile);
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  const list = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const lastKey = useRef<string | null>(null);

  // A conversation opened starts at its end; after that the list stays at the end while it was there, so a new message
  // comes into view and a list scrolled up for older messages keeps its place.
  const key = view?.target.key ?? null;
  useLayoutEffect(() => {
    const el = list.current;
    if (!el) return;
    const switched = lastKey.current !== key;
    lastKey.current = key;
    if (switched) atBottom.current = true;
    if (switched || atBottom.current) el.scrollTop = el.scrollHeight;
  });

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
      onScroll={(e) => {
        const el = e.currentTarget;
        atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
      }}
      onKeyDown={(e) => listKey(e, list.current, items, view)}
    >
      {content}
    </div>
  );
}
