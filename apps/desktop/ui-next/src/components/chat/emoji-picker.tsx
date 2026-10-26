// The reaction picker: a curated grid drawn with the system emoji font, a name search, arrow-key navigation. Opened with
// openEmojiPicker(anchor, onPick); EmojiPickerHost (mounted once in App, above the settings overlays) renders it. Ported from the old UI's views/emoji.js.
import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { create } from 'zustand';
import { EMOJI } from '@/lib/emoji';

const COLUMNS = 8;

interface Opened {
  id: number; // a new id remounts the picker, so a new opening starts with an empty search
  anchor: HTMLElement;
  opener: HTMLElement | null; // the element that gets the focus back when the picker closes
  onPick: (emoji: string) => void;
  label: string; // the dialog's name for screen readers: "Add reaction" for reactions, "Insert emoji" for the composer
}

const usePicker = create<{ open: Opened | null }>(() => ({ open: null }));

// Opens the picker beside `anchor`. onPick(emoji) runs once, after the picker closes.
let seq = 0;

export function openEmojiPicker(anchor: HTMLElement, onPick: (emoji: string) => void, label = 'Add reaction'): void {
  usePicker.setState({ open: { id: ++seq, anchor, opener: anchor, onPick, label } });
}

export function closeEmojiPicker(): void {
  const current = usePicker.getState().open;
  usePicker.setState({ open: null });
  if (current?.opener?.isConnected) current.opener.focus();
}

// Below the anchor when there is room, otherwise above it, and never off the window's edges.
function placeAt(anchor: HTMLElement, width: number, height: number): { left: number; top: number } {
  const a = anchor.getBoundingClientRect();
  const left = Math.max(8, Math.min(a.right - width, window.innerWidth - width - 8));
  const below = a.bottom + 8;
  const top = below + height <= window.innerHeight - 8 ? below : Math.max(8, a.top - 8 - height);
  return { left: Math.round(left), top: Math.round(top) };
}

export function EmojiPickerHost() {
  const open = usePicker((s) => s.open);
  if (!open) return null;
  return <Picker key={open.id} opened={open} />;
}

function Picker({ opened }: { opened: Opened }) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0); // the cell with the roving tab stop
  const node = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>({ left: 0, top: 0 });

  const q = query.trim().toLowerCase();
  const matches = EMOJI.filter(([emoji, names]) => !q || names.includes(q) || emoji === q);

  useLayoutEffect(() => {
    if (node.current) setPos(placeAt(opened.anchor, node.current.offsetWidth, node.current.offsetHeight));
    search.current?.focus();
  }, [opened.anchor]);

  // A press outside closes it. The anchor's own press (the button that opened it) does not count.
  useLayoutEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (node.current && !node.current.contains(e.target as Node)) closeEmojiPicker();
    };
    document.addEventListener('pointerdown', onDown, true);
    return () => document.removeEventListener('pointerdown', onDown, true);
  }, []);

  const pick = (emoji: string) => {
    closeEmojiPicker();
    opened.onPick(emoji);
  };

  const cells = (): HTMLButtonElement[] => [...(node.current?.querySelectorAll<HTMLButtonElement>('.emoji-cell') ?? [])];

  const onSearchKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      cells()[0]?.focus();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const first = matches[0];
      if (first) pick(first[0]);
    }
  };

  const onGridKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const all = cells();
    const i = all.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    const moves: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLUMNS, ArrowUp: -COLUMNS, Home: -i, End: all.length - 1 - i };
    if (!(e.key in moves)) return;
    e.preventDefault();
    const next = i + moves[e.key];
    if (e.key === 'ArrowUp' && next < 0) {
      search.current?.focus();
      return;
    }
    const target = Math.max(0, Math.min(all.length - 1, next));
    setActive(target);
    all[target].focus();
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeEmojiPicker();
    }
  };

  return (
    <div
      ref={node}
      className="emoji-picker"
      role="dialog"
      aria-label={opened.label}
      style={{ left: pos.left, top: pos.top }}
      onKeyDown={onKey}
    >
      <input
        ref={search}
        type="search"
        className="emoji-search"
        placeholder="Search reactions"
        aria-label="Search reactions"
        autoComplete="off"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={onSearchKey}
      />
      <div className="emoji-grid" role="group" aria-label="Reactions" onKeyDown={onGridKey}>
        {matches.length === 0 ? (
          <p className="emoji-empty muted">No matching emoji.</p>
        ) : (
          matches.map(([emoji, names], i) => (
            <button
              key={emoji}
              type="button"
              className="emoji-cell"
              tabIndex={i === active ? 0 : -1}
              aria-label={names.split(' ')[0]}
              title={names.split(' ').slice(0, 2).join(' ')}
              onFocus={() => setActive(i)}
              onClick={() => pick(emoji)}
            >
              {emoji}
            </button>
          ))
        )}
      </div>
    </div>
  );
}
