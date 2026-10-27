// The title bar's network line: the visible text, the tooltip lines with the full numbers, and whether the node is
// connected. Ported from formatNet() in views/status.js; the copy is the current UI's. The "resolved via" text is
// lib/dials.ts, which the Network settings page shows too.
import { formatDials } from '@/lib/dials';
import type { NetLatency, NetStatus } from '@/lib/types';

const IP_LIKE = /^(\d{1,3}\.){3}\d{1,3}$|^\[?[0-9a-f:]+\]?$/i;

// How a latency reads. No reading is a dash.
export type LatencyTone = 'normal' | 'warn' | 'slow' | 'none';

// A DHT lookup walks several hops, so it is far slower than a ping. Calibrated on a real network: 20 lookups from one
// machine, with 17 to 23 nodes answering each, gave a median of 2.5 s and a 90th percentile of 2.9 s (slowest 3.1 s).
// Up to 3 s is normal, 3 to 5 s is a warning, and over 5 s is slow. Re-measure with the calibrate_lookup_times test.
const LOOKUP_NORMAL_MS = 3000;
const LOOKUP_SLOW_MS = 5000;
// A relay's round trip is a ping: under 150 ms is normal, 150 to 400 ms is a warning, and over 400 ms is slow.
const PING_NORMAL_MS = 150;
const PING_SLOW_MS = 400;

export interface NetSegment {
  text: string;
  tone?: 'warn' | 'slow';
}

export interface NetView {
  segments: NetSegment[]; // the visible line, in order; the text of each is joined with " · " by the caller
  lines: string[]; // the tooltip, one line each
  ok: boolean;
}

export function latencyTone(ms: number | null | undefined, kind: 'lookup' | 'ping'): LatencyTone {
  if (ms === null || ms === undefined) return 'none';
  const [normal, slow] = kind === 'lookup' ? [LOOKUP_NORMAL_MS, LOOKUP_SLOW_MS] : [PING_NORMAL_MS, PING_SLOW_MS];
  if (ms <= normal) return 'normal';
  if (ms <= slow) return 'warn';
  return 'slow';
}

// "2529 ms lookup" for a DHT lookup, "42 ms" for a relay round trip, or a dash when there is no reading.
export function latencyText(ms: number | null | undefined, kind: 'lookup' | 'ping'): string {
  if (ms === null || ms === undefined) return '—';
  return kind === 'lookup' ? `${Math.round(ms)} ms lookup` : `${Math.round(ms)} ms`;
}

// The colour a latency segment gets. Normal text and a dash take no colour, so the bar stays quiet.
function toneClass(ms: number | null | undefined, kind: 'lookup' | 'ping'): NetSegment['tone'] {
  const tone = latencyTone(ms, kind);
  if (tone === 'warn') return 'warn';
  if (tone === 'slow') return 'slow';
  return undefined;
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

// `latest` is the newest reading from the net_latency event. It wins over the poll's values, which are kept as the first
// reading until the first probe.
export function formatNet(status: NetStatus, latest: NetLatency | null = null): NetView {
  const host = relayHost(status.relay);
  const dials = formatDials(status.dials);
  const dialsLine = dials ? `resolved via ${dials}` : null;
  const dht = status.dht;
  if (!dht) {
    const text = `DHT offline · ${status.peers} peers`;
    return { segments: [{ text }], lines: [text, dialsLine].filter((l): l is string => l !== null), ok: false };
  }
  const dhtMs = latest ? latest.dht_ms : dht.latency_ms ?? null;
  const relayMs = latest ? latest.relay_ms : status.relay_ms ?? null;
  const reach = dht.firewalled ? 'firewalled' : 'reachable';
  const segments: NetSegment[] = [
    { text: `DHT ≈ ${formatCount(dht.size_estimate)} nodes` },
    { text: latencyText(dhtMs, 'lookup'), tone: toneClass(dhtMs, 'lookup') },
    { text: reach },
    { text: `${status.peers} peers` },
  ];
  if (host) {
    segments.push({ text: `relay ${host}` });
    if (relayMs !== null && relayMs !== undefined) segments.push({ text: latencyText(relayMs, 'ping'), tone: toneClass(relayMs, 'ping') });
  }
  const confidence = Math.round(dht.confidence * 100);
  const title = [
    `DHT size estimate ${Math.round(dht.size_estimate).toLocaleString()} nodes`,
    `confidence ${confidence}%`,
    reach,
    `${status.peers} peers`,
    host ? `relay ${host}` : null,
  ].filter(Boolean).join(' · ');
  // A lookup is the time to walk the DHT to its end, not one round trip. The bar shows the median of the last lookups.
  const latencyLine = dhtMs === null || dhtMs === undefined
    ? 'DHT lookup time: no lookup has been answered yet.'
    : `DHT lookup time: ${latencyText(dhtMs, 'lookup')}, the median of the last lookups (about two minutes). Under 3 s is normal, 3 to 5 s is a warning, over 5 s is slow.`;
  const relayLine = host && relayMs !== null && relayMs !== undefined ? `Relay round trip: ${latencyText(relayMs, 'ping')}.` : null;
  return {
    segments,
    lines: [title, latencyLine, relayLine, dialsLine].filter((l): l is string => l !== null),
    ok: true,
  };
}
