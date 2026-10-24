// Last-seen markers for channels and DMs, kept in localStorage. Storage can be unavailable (private windows,
// blocked site data), so every access is guarded and the app still works without it.
// Keys: `ch:<communityId>:<channelId>` and `dm:<peerRootId>`. Values: { id, ms } of the newest message seen.
const KEY = 'dissent.seen.v1';

function read() {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch {
    return {};
  }
}

function write(map) {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(map));
  } catch {
    // Unavailable: markers only last for this session.
  }
}

// Timestamps are compared, not ids: message ids are not ordered.
export function seenMs(key) {
  const entry = read()[key];
  return entry && typeof entry.ms === 'number' ? entry.ms : null;
}

export function markSeen(key, id, ms) {
  const map = read();
  if (!map[key] || map[key].ms < ms) {
    map[key] = { id, ms };
    write(map);
  }
}
