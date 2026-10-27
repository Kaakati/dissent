// Server boosts: the level a server's relay hosts earn, as this device verifies them. Nothing is paid and nothing basic
// is gated; a level only raises call ceilings. See docs/design/12-boosts.md.

// "Level 2 · 3 relay hosts", the line the sidebar title, the invite card and Server Settings show.
export function boostLabel(level: number, hosts: number): string {
  return `Level ${level} · ${hosts} relay host${hosts === 1 ? '' : 's'}`;
}

// "1080p30", how a ceiling reads.
export const videoLabel = (height: number, fps: number): string => `${height}p${fps}`;
