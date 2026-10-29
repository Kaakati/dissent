// The server's channel layout as the sidebar edits it: channels outside any category, and categories holding channels
// of either kind, top first. The engine keeps one layout per server (`set_channel_layout`); a server that has none
// shows two built-in groups, Text channels and Voice channels, which become real categories the first time someone
// arranges the channels. Every function here is pure: it takes entries and returns new ones.
import type { CommunityView, Id, LayoutEntry, LayoutCategory } from './types';

// The key a category goes by in the sidebar. A category the engine has not named yet (a built-in group, or one just
// made here) has a local key, which never reaches the engine.
export const VIRTUAL_TEXT = 'virtual:text';
export const VIRTUAL_VOICE = 'virtual:voice';
const LOCAL = 'new:';
let localSeq = 0;

export const keyOf = (c: LayoutCategory): string => c.id ?? '';

// A top-level entry's key: its channel id, or its category's key, with a prefix so the two never clash.
export const topKey = (e: LayoutEntry): string => (e.kind === 'channel' ? `c:${e.id}` : `k:${e.id ?? ''}`);
const isLocal = (id: string | null | undefined) => !id || id.startsWith(LOCAL) || id.startsWith('virtual:');

// The entries the sidebar shows: the server's layout, or the two built-in groups when it has none.
export function layoutOf(community: CommunityView): LayoutEntry[] {
  if (community.layout) return community.layout;
  const ids = (voice: boolean) => community.channels.filter((c) => Boolean(c.voice) === voice).map((c) => c.id);
  return [
    { kind: 'category', id: VIRTUAL_TEXT, name: 'Text channels', collapsed: false, channels: ids(false) },
    { kind: 'category', id: VIRTUAL_VOICE, name: 'Voice channels', collapsed: false, channels: ids(true) },
  ];
}

// The layout as `set_channel_layout` takes it: a category with a local key goes without an id, so the engine makes one.
export function toPayload(entries: LayoutEntry[]): LayoutEntry[] {
  return entries.map((e) =>
    e.kind === 'category' && isLocal(e.id) ? { kind: 'category', name: e.name, collapsed: e.collapsed ?? false, channels: [...e.channels] } : e,
  );
}

// Every channel top to bottom, categories opened in place.
export const flatten = (entries: LayoutEntry[]): Id[] => entries.flatMap((e) => (e.kind === 'channel' ? [e.id] : e.channels));

// Where a channel is: its category's key (null outside any) and its place there.
export function placeOf(entries: LayoutEntry[], channel: Id): { category: string | null; index: number } | null {
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    if (e.kind === 'channel' && e.id === channel) return { category: null, index: i };
    if (e.kind === 'category') {
      const at = e.channels.indexOf(channel);
      if (at >= 0) return { category: e.id ?? null, index: at };
    }
  }
  return null;
}

// The layout without `channel`, wherever it was.
export const withoutChannel = (entries: LayoutEntry[], channel: Id): LayoutEntry[] =>
  entries
    .filter((e) => !(e.kind === 'channel' && e.id === channel))
    .map((e) => (e.kind === 'category' ? { ...e, channels: e.channels.filter((c) => c !== channel) } : e));

// `channel` moved into `category` (null for outside any) at `index` of that list. For the top level the index counts
// the top-level entries, categories included, after the channel is taken out.
export function moveChannel(entries: LayoutEntry[], channel: Id, category: string | null, index: number): LayoutEntry[] {
  const rest = withoutChannel(entries, channel);
  if (category === null) {
    const out = [...rest];
    out.splice(Math.max(0, Math.min(index, out.length)), 0, { kind: 'channel', id: channel });
    return out;
  }
  return rest.map((e) => {
    if (e.kind !== 'category' || e.id !== category) return e;
    const channels = [...e.channels];
    channels.splice(Math.max(0, Math.min(index, channels.length)), 0, channel);
    return { ...e, channels };
  });
}

// A category moved to `index` of the top level, counted after it is taken out.
export function moveCategory(entries: LayoutEntry[], category: string, index: number): LayoutEntry[] {
  const at = entries.findIndex((e) => e.kind === 'category' && e.id === category);
  if (at < 0) return entries;
  const out = [...entries];
  const [moved] = out.splice(at, 1);
  out.splice(Math.max(0, Math.min(index, out.length)), 0, moved);
  return out;
}

// One step up or down for a channel, through every place it can be: within its category, out of it at either end, and
// into the category next to it. `null` when it is already at the top or the bottom.
export function stepChannel(entries: LayoutEntry[], channel: Id, dir: -1 | 1): LayoutEntry[] | null {
  const place = placeOf(entries, channel);
  if (!place) return null;
  if (place.category !== null) {
    const top = entries.findIndex((e) => e.kind === 'category' && e.id === place.category);
    const cat = entries[top] as LayoutCategory & { kind: 'category' };
    const to = place.index + dir;
    if (to >= 0 && to < cat.channels.length) return moveChannel(entries, channel, place.category, to);
    // Out of the category: just above it, or just below it.
    return moveChannel(entries, channel, null, dir < 0 ? top : top + 1);
  }
  const neighbour = entries[place.index + dir];
  if (!neighbour) return null;
  if (neighbour.kind === 'category') {
    // Into the category next to it: at its end from below, at its start from above.
    return moveChannel(entries, channel, neighbour.id ?? null, dir < 0 ? neighbour.channels.length : 0);
  }
  return moveChannel(entries, channel, null, place.index + dir);
}

// One step up or down for a category among the top-level entries. `null` at either end.
export function stepCategory(entries: LayoutEntry[], category: string, dir: -1 | 1): LayoutEntry[] | null {
  const at = entries.findIndex((e) => e.kind === 'category' && e.id === category);
  const to = at + dir;
  if (at < 0 || to < 0 || to >= entries.length) return null;
  return moveCategory(entries, category, to);
}

// A new, empty category at the end, with a local key until the engine names it.
export function addCategory(entries: LayoutEntry[], name: string): LayoutEntry[] {
  localSeq += 1;
  return [...entries, { kind: 'category', id: `${LOCAL}${localSeq}`, name: name.trim(), collapsed: false, channels: [] }];
}

export function renameCategory(entries: LayoutEntry[], category: string, name: string): LayoutEntry[] {
  return entries.map((e) => (e.kind === 'category' && e.id === category ? { ...e, name: name.trim() } : e));
}

// A deleted category's channels stay, outside any category, where the category was.
export function deleteCategory(entries: LayoutEntry[], category: string): LayoutEntry[] {
  return entries.flatMap((e): LayoutEntry[] =>
    e.kind === 'category' && e.id === category ? e.channels.map((id) => ({ kind: 'channel', id })) : [e],
  );
}

// The categories, for a "Move to category" menu.
export const categoriesOf = (entries: LayoutEntry[]): (LayoutCategory & { kind: 'category' })[] =>
  entries.filter((e): e is LayoutCategory & { kind: 'category' } => e.kind === 'category');

export const MAX_CATEGORY_NAME = 64;
export const MAX_CATEGORIES = 50;
