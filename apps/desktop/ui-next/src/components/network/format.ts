// The title bar's network line: the visible text, the tooltip lines with the full numbers, and whether the node is
// connected. Ported from formatNet() in views/status.js; the copy is the current UI's. The "resolved via" text is
// lib/dials.ts, which the Network settings page shows too.
import { formatDials } from '@/lib/dials';
import type { NetStatus } from '@/lib/types';

const IP_LIKE = /^(\d{1,3}\.){3}\d{1,3}$|^\[?[0-9a-f:]+\]?$/i;

export interface NetView {
  text: string;
  lines: string[]; // the tooltip, one line each
  ok: boolean;
}

// Shows only the relay host, never an address. Null when the value has no usable hostname.
export function relayHost(relay: string | null | undefined): string | null {
  if (!relay) return null;
  try {
    // The engine sends a bare hostname; a scheme is added only so URL can parse it.
    const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(relay) ? relay : `https://${relay}`;
    const host = new URL(withScheme).hostname.replace(/\.$/, '');
    return host && !IP_LIKE.test(host) ? host : null;
  } catch {
    return null;
  }
}

function formatCount(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}

export function formatNet(status: NetStatus): NetView {
  const host = relayHost(status.relay);
  const dials = formatDials(status.dials);
  const dialsLine = dials ? `resolved via ${dials}` : null;
  const dht = status.dht;
  if (!dht) {
    const text = `DHT offline · ${status.peers} peers`;
    return { text, lines: [text, dialsLine].filter((l): l is string => l !== null), ok: false };
  }
  const reach = dht.firewalled ? 'firewalled' : 'reachable';
  const parts = [`DHT ≈ ${formatCount(dht.size_estimate)} nodes`, reach, `${status.peers} peers`];
  if (host) parts.push(`relay ${host}`);
  const confidence = Math.round(dht.confidence * 100);
  const title = [
    `DHT size estimate ${Math.round(dht.size_estimate).toLocaleString()} nodes`,
    `confidence ${confidence}%`,
    reach,
    `${status.peers} peers`,
    host ? `relay ${host}` : null,
  ].filter(Boolean).join(' · ');
  return { text: parts.join(' · '), lines: [title, dialsLine].filter((l): l is string => l !== null), ok: true };
}
