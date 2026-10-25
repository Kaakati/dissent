// The "resolved via" line: how the last connections were dialled. Ported from formatDials in views/status.js, which
// the status view and the Network settings page both show.
import type { NetDials } from './types';

const DIAL_SOURCES = ['cache', 'peer', 'seeder', 'lan', 'dht', 'open'] as const;

// Null until the engine reports dials. A zero on dht means the Mainline DHT was not needed.
export function formatDials(dials: NetDials | null | undefined): string | null {
  if (!dials) return null;
  const counts = DIAL_SOURCES.map((source) => `${source} ${dials.by_source?.[source] ?? 0}`).join(' · ');
  const last = dials.last_via ? ` · last ${dials.last_via}, ${dials.last_connect_ms} ms` : '';
  const failed = dials.failed ? ` · ${dials.failed} failed` : '';
  return `${counts}${last}${failed}`;
}
