// A direct or group call, laid out as Discord does: the call stage on top and the conversation below it, the full chat
// with its composer, split by a divider that drags, takes the arrow keys, and resets on a double-click. The stage can
// collapse to a strip with the people and the controls. The split and the collapse are kept on this device.
import { useCallback, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { ChatBody } from '@/components/chat/chat-body';
import type { CallState } from '@/lib/types';
import { CallStage } from './call-body';

const SPLIT_KEY = 'dissent.dmCallSplit'; // the stage's share of the pane, 0..1
const COLLAPSED_KEY = 'dissent.dmCallCollapsed';
export const DEFAULT_SPLIT = 0.55;
const STAGE_MIN_PX = 180;
const CHAT_MIN_PX = 220;
const DIVIDER_PX = 8;
const KEY_STEP = 0.05;

function readNumber(key: string, fallback: number): number {
  try {
    const v = Number(localStorage.getItem(key));
    return localStorage.getItem(key) !== null && Number.isFinite(v) && v > 0 && v < 1 ? v : fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A window without storage keeps the choice for the session.
  }
}

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === 'true';
  } catch {
    return false;
  }
}

// The share the stage may take in a pane of `height` px: both sides keep their minimum. A pane too short for both
// gives the chat its minimum first.
export function splitBounds(height: number): { min: number; max: number } {
  const usable = Math.max(1, height - DIVIDER_PX);
  const min = Math.min(STAGE_MIN_PX / usable, 1);
  const max = Math.max(min, 1 - CHAT_MIN_PX / usable);
  return { min: Math.min(min, max), max };
}

const clamp = (v: number, { min, max }: { min: number; max: number }) => Math.min(max, Math.max(min, v));

export function DmCallSplit({ call }: { call: CallState }) {
  const root = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);
  const [split, setSplit] = useState(() => readNumber(SPLIT_KEY, DEFAULT_SPLIT));
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const drag = useRef<{ pointerId: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => setHeight(entry.contentRect.height));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const bounds = splitBounds(height);
  const shown = clamp(split, bounds);
  const commit = useCallback((next: number) => {
    setSplit(next);
    write(SPLIT_KEY, String(next));
  }, []);
  const toggleCollapsed = useCallback(() => {
    setCollapsed((c) => {
      write(COLLAPSED_KEY, String(!c));
      return !c;
    });
  }, []);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { pointerId: e.pointerId, top: root.current?.getBoundingClientRect().top ?? 0 };
  };
  // The split with the divider's middle at the pointer.
  const splitAt = (clientY: number, top: number) => clamp((clientY - top - DIVIDER_PX / 2) / Math.max(1, height - DIVIDER_PX), bounds);
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    setSplit(splitAt(e.clientY, d.top));
  };
  // The release is placed where the pointer ends, so a move the browser coalesced away still lands.
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    drag.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
    commit(splitAt(e.clientY, d.top));
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const next = ({ ArrowUp: shown - KEY_STEP, ArrowDown: shown + KEY_STEP, Home: bounds.min, End: bounds.max } as Record<string, number>)[e.key];
    if (next === undefined) return;
    e.preventDefault();
    commit(clamp(next, bounds));
  };

  const stagePx = Math.round(shown * Math.max(0, height - DIVIDER_PX));
  return (
    <div ref={root} className="flex min-h-0 min-w-0 flex-col" data-call-layout="split" data-call-collapsed={collapsed ? 'true' : 'false'}>
      <div
        className="flex min-h-0 shrink-0 flex-col p-s-2 pb-0"
        style={collapsed ? undefined : { height: height ? stagePx : `${DEFAULT_SPLIT * 100}%` }}
        data-call-stage
      >
        <CallStage call={call} compact={collapsed} onToggleCompact={toggleCollapsed} />
      </div>
      {collapsed ? (
        <div className="h-s-2 shrink-0" aria-hidden="true" />
      ) : (
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize the call"
          aria-valuenow={Math.round(shown * 100)}
          aria-valuemin={Math.round(bounds.min * 100)}
          aria-valuemax={Math.round(bounds.max * 100)}
          tabIndex={0}
          title="Drag to resize. Double-click to reset."
          className="group grid shrink-0 cursor-row-resize touch-none place-items-center focus-visible:outline-2 focus-visible:outline-ring"
          style={{ height: DIVIDER_PX }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDoubleClick={() => commit(clamp(DEFAULT_SPLIT, bounds))}
          onKeyDown={onKeyDown}
          data-call-divider
        >
          <span className="h-1 w-10 rounded-full bg-border group-hover:bg-muted-foreground group-focus-visible:bg-muted-foreground" aria-hidden="true" />
        </div>
      )}
      <ChatBody />
    </div>
  );
}
