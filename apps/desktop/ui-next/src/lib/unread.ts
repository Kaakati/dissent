// Last-seen markers for channels and DMs, kept in localStorage. Storage can be unavailable (private windows, blocked
// site data), so every access is guarded and the app still works without it. Ported from ui/unread.js; the key and the
// stored shape are the same, so both UIs read one marker set.
// Keys: `ch:<communityId>:<channelId>` and `dm:<peerRootId>`. Values: { id, ms } of the newest message seen.

const KEY = 'dissent.seen.v1';

interface Marker {
  id: string;
  ms: number;
}

function read(): Record<string, Marker> {
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, Marker>) : {};
  } catch {
    return {};
  }
}

function write(map: Record<string, Marker>): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(map));
  } catch {
    // Unavailable: markers only last for this session.
  }
}

// Timestamps are compared, not ids: message ids are not ordered.
export function seenMs(key: string): number | null {
  const entry = read()[key];
  return entry && typeof entry.ms === 'number' ? entry.ms : null;
}

export function markSeen(key: string, id: string, ms: number): void {
  const map = read();
  if (!map[key] || map[key].ms < ms) {
    map[key] = { id, ms };
    write(map);
  }
}
