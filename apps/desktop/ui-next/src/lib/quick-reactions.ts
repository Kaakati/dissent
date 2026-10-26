// The quick reactions of the message menu: the viewer's most-used emoji, then the defaults. Counts are kept in
// localStorage, a per-viewer convenience only: a read or write that fails just leaves the defaults.
const KEY = 'dissent.reactions.v1';
const DEFAULTS = ['😆', '👍', '💡', '✅'];
const COUNT = 4;

function load(): Record<string, number> {
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, number>) : {};
  } catch {
    return {};
  }
}

// Counts one reaction the viewer added.
export function noteReaction(emoji: string): void {
  try {
    const counts = load();
    counts[emoji] = (Number(counts[emoji]) || 0) + 1;
    window.localStorage.setItem(KEY, JSON.stringify(counts));
  } catch {
    // Unavailable: the defaults stay.
  }
}

// The four emoji of the quick row: the most used first, then the defaults not already in the row.
export function quickReactions(): string[] {
  const counts = load();
  const used = Object.entries(counts)
    .filter(([, n]) => Number(n) > 0)
    .sort((a, b) => Number(b[1]) - Number(a[1]))
    .map(([emoji]) => emoji)
    .slice(0, COUNT);
  for (const emoji of DEFAULTS) {
    if (used.length >= COUNT) break;
    if (!used.includes(emoji)) used.push(emoji);
  }
  return used;
}
