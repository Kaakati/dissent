// Network and DHT stats in the title bar's right corner: a status dot and one line of text. Polls every 15 seconds.
import { api } from '../api.js';
import { el } from '../ui.js';

const POLL_MS = 15000;
const IP_LIKE = /^(\d{1,3}\.){3}\d{1,3}$|^\[?[0-9a-f:]+\]?$/i;

// Shows only the relay host, never an address. Returns null when the value has no usable hostname.
function relayHost(relay) {
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

function formatCount(n) {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}

// Returns the visible text, the tooltip with the full numbers, and whether the node is connected.
export function formatNet(status) {
  const host = relayHost(status.relay);
  const dht = status.dht;
  if (!dht) {
    return { text: `DHT offline · ${status.peers} peers`, title: `DHT offline · ${status.peers} peers`, ok: false };
  }
  const reach = dht.firewalled ? 'firewalled' : 'reachable';
  const parts = [`DHT ≈ ${formatCount(dht.size_estimate)} nodes`, reach, `${status.peers} peers`];
  if (host) parts.push(`relay ${host}`);
  const confidence = Math.round(dht.confidence * 100);
  const title = [
    `DHT size estimate ${Math.round(dht.size_estimate).toLocaleString()} nodes`,
    `confidence ${confidence}%`,
    dht.firewalled ? 'firewalled' : 'reachable',
    `${status.peers} peers`,
    host ? `relay ${host}` : null,
  ].filter(Boolean).join(' · ');
  return { text: parts.join(' · '), title, ok: true };
}

export function initStatus() {
  const host = document.getElementById('net-status');
  // The status line is not clickable, so its parts drag the window (Tauri checks the clicked element itself).
  const dot = el('span', { class: 'net-dot', 'aria-hidden': 'true', 'data-tauri-drag-region': '' });
  const text = el('span', { 'data-tauri-drag-region': '' });
  host.replaceChildren(dot, text);
  const refresh = async () => {
    try {
      const s = formatNet(await api.netStatus());
      text.textContent = s.text;
      host.title = s.title;
      dot.classList.toggle('ok', s.ok);
      host.hidden = false;
    } catch {
      host.hidden = true;
    }
  };
  refresh();
  setInterval(refresh, POLL_MS);
}
