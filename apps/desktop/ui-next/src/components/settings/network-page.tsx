// Settings > Network: the live status of the connection, the relays (public ones on or off, and your own), and the local
// network switch. Saving restarts the engine. The status refreshes every 15 seconds while the page is open. Ported from
// the network section of views/settings.js.
import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api } from '@/lib/api';
import { formatDials } from '@/lib/dials';
import { toast } from '@/lib/toasts';
import { useShell } from '@/lib/store';
import type { NetStatus, NetworkSettings } from '@/lib/types';

const REFRESH_MS = 15000;
const MAX_RELAYS = 8;

export function NetworkPage() {
  const setNetworkSettings = useShell((s) => s.setNetworkSettings);
  const [settings, setSettings] = useState<NetworkSettings>({ use_public_relays: true, relays: [] });
  const [loaded, setLoaded] = useState(false);
  const [url, setUrl] = useState('');
  const [status, setStatus] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    api
      .getNetworkSettings()
      .then((s) => {
        setSettings(s);
        setLoaded(true);
      })
      .catch((err) => toast(err));
  }, []);

  const empty = !settings.use_public_relays && settings.relays.length === 0;

  const add = () => {
    let parsed: URL;
    try {
      parsed = new URL(url.trim());
    } catch {
      setStatus('Enter a full URL, for example https://relay.example.org');
      return;
    }
    if (parsed.protocol !== 'https:') {
      setStatus('A relay URL must use https.');
      return;
    }
    if (settings.relays.length >= MAX_RELAYS) {
      setStatus(`Up to ${MAX_RELAYS} custom relays.`);
      return;
    }
    const text = parsed.toString();
    if (!settings.relays.includes(text)) setSettings({ ...settings, relays: [...settings.relays, text] });
    setUrl('');
    setStatus('');
  };

  const save = async () => {
    setSaving(true);
    const saved = await setNetworkSettings(settings);
    setSaving(false);
    if (saved) setSettings(saved);
  };

  return (
    <div className="flex flex-col">
      <Block title="Status">
        <NetTable />
      </Block>

      <Block title="Relays">
        <CheckRow>
          <input
            type="checkbox"
            aria-label="Use public n0 relays"
            checked={settings.use_public_relays}
            onChange={(e) => setSettings({ ...settings, use_public_relays: e.target.checked })}
            className={CHECK_CLASS}
          />
          <span>Use public n0 relays</span>
        </CheckRow>
        <Hint>
          With public relays on, your relays are added to number0’s. With it off, only your relays are used, and you need at least one. Each must be an https URL.
        </Hint>
        {settings.relays.length ? (
          <ul aria-label="Custom relays" className="m-0 list-none p-0">
            {settings.relays.map((relay) => (
              <li key={relay} className="flex min-h-8 items-center justify-between gap-s-2 break-all">
                <span className="font-mono text-meta">{relay}</span>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${relay}`}
                  title="Remove"
                  onClick={() => setSettings({ ...settings, relays: settings.relays.filter((r) => r !== relay) })}
                >
                  <X />
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
        <div className="flex items-center gap-s-2">
          <Input placeholder="https://relay.example.org" aria-label="Relay URL" value={url} onChange={(e) => setUrl(e.target.value)} />
          <Button variant="outline" onClick={add}>
            Add relay
          </Button>
        </div>
        {status ? <Hint role="status">{status}</Hint> : null}
        {empty ? <Hint className="text-warn" role="status">Add at least one relay, or turn public relays back on.</Hint> : null}
        <div className="flex justify-end">
          <Button disabled={empty || saving || !loaded} onClick={() => void save()}>
            Save network settings
          </Button>
        </div>
      </Block>

      <Block title="Local network">
        <CheckRow>
          <input
            type="checkbox"
            aria-label="Find people on the same network"
            checked={Boolean(settings.lan_discovery)}
            onChange={(e) => setSettings({ ...settings, lan_discovery: e.target.checked })}
            className={CHECK_CLASS}
          />
          <span>Find people on the same network</span>
        </CheckRow>
        <Hint>
          Off by default. While it is on, your device announces a token that changes every ten minutes, so others on the network cannot tell which community or device it belongs to. Devices exchange identities only after both prove they share a community. Saving restarts the connection.
        </Hint>
      </Block>
    </div>
  );
}

// The status table. A failed read shows "Status unavailable"; the next refresh tries again.
function NetTable() {
  const [net, setNet] = useState<NetStatus | 'failed' | null>(null);
  useEffect(() => {
    let live = true;
    const refresh = () => {
      api
        .netStatus()
        .then((s) => live && setNet(s))
        .catch(() => live && setNet('failed'));
    };
    refresh();
    const timer = setInterval(refresh, REFRESH_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);

  if (net === null) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (net === 'failed') return <p className="text-sm text-text-strong">Status unavailable</p>;
  const rows: [string, string][] = [
    ['DHT size', net.dht ? `≈ ${Math.round(net.dht.size_estimate).toLocaleString('en-US')} nodes` : 'offline'],
    ['Confidence', net.dht ? `${Math.round(net.dht.confidence * 100)}%` : '—'],
    ['Reachability', net.dht ? (net.dht.firewalled ? 'firewalled' : 'reachable') : '—'],
    ['Peers', String(net.peers)],
    ['Relay', net.relay ?? 'none'],
    ['Resolved via', formatDials(net.dials) ?? 'no dials yet'],
  ];
  return (
    <dl aria-label="Network status" className="m-0 grid grid-cols-[160px_1fr] gap-x-s-4 gap-y-s-1 text-sm tabular-nums">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-muted-foreground">{label}</dt>
          <dd className="m-0 text-text-strong break-words">{value}</dd>
        </div>
      ))}
    </dl>
  );
}
