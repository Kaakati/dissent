// Small pure helpers for names, colours and counts. Ported from the old UI's ui.js.

// The hue of an avatar or server is a stable hash of its id, so one person keeps one colour everywhere.
export function hueOf(seed: string | null | undefined): number {
  // The mock fixes the hue of its people, so screenshots and the driver see the same colours as the current UI.
  const forced = (globalThis as { __mockHues?: Record<string, number> }).__mockHues?.[String(seed)];
  if (forced !== undefined) return forced;
  let h = 0x811c9dc5;
  for (const ch of String(seed ?? '')) h = Math.imul(h ^ (ch.codePointAt(0) ?? 0), 0x01000193);
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  return (h >>> 0) % 360;
}

// The background of an avatar, rail button or server mark: the same oklch the current UI uses.
export function hueBackground(hue: number): string {
  return `oklch(48% 0.14 ${hue})`;
}

// Up to two letters: the first two words, or the first and last of three or more ("Open Source Arabic" is OA).
export function initials(name: string | null | undefined): string {
  const parts = String(name ?? '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return [...parts[0]].slice(0, 2).join('').toUpperCase();
  const picks = parts.length > 2 ? [parts[0], parts[parts.length - 1]] : parts.slice(0, 2);
  return picks.map((p) => [...p][0]).join('').toUpperCase();
}

export const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`;
