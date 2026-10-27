// Settings > Network: the live status of the connection, the relays the node uses (where each came from, the latency measured to it, and the
// home relay), the relays you add, and the local network switch. Saving restarts the engine. The status and the relay list refresh every
// 15 seconds while the page is open. Ported from the network section of views/settings.js.
import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, X } from 'lucide-react';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api } from '@/lib/api';
import { formatDials } from '@/lib/dials';
import { toast } from '@/lib/toasts';
import { useShell } from '@/lib/store';
import type { NetStatus, NetworkSettings, RelayRow, RelayStatus } from '@/lib/types';

const REFRESH_MS = 15000;
// Matches MAX_RELAYS in apps/desktop/src-tauri/src/network.rs: the relays this person adds.
const MAX_RELAYS = 16;
const EMPTY_OFF = 'No relay is enabled. Add one, turn public relays back on, or re-enable one.';
const SOURCE_LABEL: Record<RelayRow['source'], string> = { custom: 'Yours', community: 'Community', hosted: 'Hosted', public: 'number0' };
// A hosted relay names its host. The verified mark shows only when this app counts that host.
const sourceText = (row: RelayRow) => (row.source === 'hosted' && row.host ? `Hosted by ${row.host}` : SOURCE_LABEL[row.source]);

// Adds the URL to the list, or removes it when it is already there.
const toggle = (list: string[] | undefined, url: string): string[] => {
  const current = list ?? [];
  return current.includes(url) ? current.filter((u) => u !== url) : [...current, url];
};

// Where the guide for running a relay for your servers lives.
const HOSTING_GUIDE = 'https://github.com/Kaakati/dissent/blob/main/packaging/README.md';
const SELECT_CLASS = 'min-h-8 w-full min-w-0 rounded-lg border border-input-border bg-input px-s-2 text-ui text-text-strong focus-visible:outline-2 focus-visible:outline-accent';

// A check's outcome: the latency when the member's app reached the relay, or the reason it did not.
type CheckResult = { url: string; ms: number } | { url: string; reason: string };

// Settings > Network > Host a relay: a member's relay for one of your servers. The check is feedback only. Publishing
// unlocks only after a pass for the same address, and then it replaces the relays you publish for that server.
function HostRelayBlock() {
  const communities = useShell((s) => s.communities);
  const [picked, setPicked] = useState('');
  const [hostUrl, setHostUrl] = useState('');
  const [checked, setChecked] = useState<CheckResult | null>(null);
  const [busy, setBusy] = useState(false);
  const communityId = communities.some((c) => c.id === picked) ? picked : (communities[0]?.id ?? '');
  const url = hostUrl.trim();
  const passed = checked && 'ms' in checked && checked.url === url ? checked : null;

  const check = async () => {
    setBusy(true);
    try {
      const ms = await api.checkRelay(communityId, url);
      setChecked({ url, ms });
    } catch (err) {
      setChecked({ url, reason: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const publish = async () => {
    if (!passed) return;
    try {
      await api.setHostedRelays(communityId, [passed.url]);
      toast('Published. Members of this server can use the relay.');
    } catch (err) {
      toast(err);
    }
  };

  return (
    <Block title="Host a relay for your servers">
      <Hint>
        A relay you run carries encrypted traffic for the members of a server you belong to. Run it with the relay service on a
        machine with a domain name and a valid certificate. The <a className="underline" href={HOSTING_GUIDE} target="_blank" rel="noreferrer">guide</a> covers the setup.
      </Hint>
      {communities.length === 0 ? (
        <Hint>Join a server to host a relay for it.</Hint>
      ) : (
        <>
          <label className="flex min-w-0 flex-col gap-s-1 text-sm leading-[19px] text-text-strong">
            <span className="text-muted-foreground">Server</span>
            <select
              aria-label="Server to host for"
              value={communityId}
              onChange={(e) => {
                setPicked(e.target.value);
                setChecked(null);
              }}
              className={SELECT_CLASS}
            >
              {communities.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </label>
          <div className="flex items-center gap-s-2">
            <Input
              placeholder="https://relay.example.org"
              aria-label="Hosted relay URL"
              value={hostUrl}
              onChange={(e) => setHostUrl(e.target.value)}
            />
            <Button variant="outline" disabled={busy || !url} onClick={check}>
              Check my relay
            </Button>
          </div>
          {checked ? (
            <Hint role="status">
              {'ms' in checked
                ? `Reachable, checked by a member just now (${checked.ms} ms).`
                : `Not checked: ${checked.reason}`}
            </Hint>
          ) : null}
          <div className="flex justify-end">
            <Button variant="outline" disabled={!passed} onClick={publish}>
              Publish to this server
            </Button>
          </div>
          <Hint>
            The check is feedback only: it never counts toward a server's level. Publishing replaces the relays you publish for
            this server with this one.
          </Hint>
        </>
      )}
    </Block>
  );
}

export function NetworkPage() {
  const setNetworkSettings = useShell((s) => s.setNetworkSettings);
  const [settings, setSettings] = useState<NetworkSettings>({
    use_public_relays: true,
    relays: [],
    pinned: [],
    disabled: [],
    use_community_relays: true,
    use_n0_lookup: true,
  });
  const [loaded, setLoaded] = useState(false);
  // Bumped after a save, so the relay list reads the new state at once.
  const [version, setVersion] = useState(0);
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
    if (saved) {
      setSettings(saved);
      setVersion((v) => v + 1);
    }
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
            aria-label="Use relays offered by my servers"
            checked={settings.use_community_relays ?? true}
            onChange={(e) => setSettings({ ...settings, use_community_relays: e.target.checked })}
            className={CHECK_CLASS}
          />
          <span>Use relays offered by my servers</span>
        </CheckRow>
        <Hint>
          Each relay you use can see your IP address. Off, your servers' relays are neither measured nor used, and only your relays and the defaults are.
        </Hint>
        <RelayList key={version} settings={settings} onChange={setSettings} />
      </Block>

      <HostRelayBlock />

      <Block title="Add or turn off relays">
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
        <CheckRow>
          <input
            type="checkbox"
            aria-label="Find devices through number0’s directory"
            checked={settings.use_n0_lookup ?? true}
            onChange={(e) => setSettings({ ...settings, use_n0_lookup: e.target.checked })}
            className={CHECK_CLASS}
          />
          <span>Find devices through number0’s directory</span>
        </CheckRow>
        <Hint>
          Faster to connect. number0 sees your device ID and addresses.
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
        {empty ? <Hint className="text-warn" role="status">{EMPTY_OFF}</Hint> : null}
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

// The relays the node uses, fastest first, with where each came from and the latency measured to it. Pin and Turn off change the
// settings; they take effect when Save network settings is pressed. Refresh re-reads the measurements. The node measures on its own
// schedule and picks the home relay itself, so a refresh may show the same home. A relay whose certificate does not check out is never
// used, and shows no latency.
function RelayList({ settings, onChange }: { settings: NetworkSettings; onChange: (next: NetworkSettings) => void }) {
  const [relays, setRelays] = useState<RelayStatus | 'failed' | null>(null);
  const load = useCallback(() => {
    api
      .relayStatus()
      .then(setRelays)
      .catch(() => setRelays('failed'));
  }, []);
  useEffect(() => {
    load();
    const timer = setInterval(load, REFRESH_MS);
    return () => clearInterval(timer);
  }, [load]);

  if (relays === null) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (relays === 'failed') return <p className="text-sm text-text-strong">Relay status unavailable</p>;

  const pinned = settings.pinned ?? [];
  const disabled = settings.disabled ?? [];
  // The same limit the desktop settings check enforces: a relay is pinned or turned off, never both.
  const pinFull = pinned.length >= PIN_LIMIT;
  return (
    <div className="flex flex-col gap-s-2">
      <div className="flex items-center justify-between gap-s-2">
        <p className="m-0 text-sm text-text-strong">Home relay: {relays.home ?? 'none yet'}</p>
        <Button variant="outline" size="sm" onClick={load}>
          <RefreshCw />
          Refresh
        </Button>
      </div>
      {relays.relays.length === 0 ? (
        <Hint>No relays yet. They appear once the node starts.</Hint>
      ) : (
        <ul aria-label="Relays" className="m-0 list-none p-0">
          {relays.relays.map((row) => {
            const off = disabled.includes(row.url);
            const isPinned = pinned.includes(row.url);
            const state = off ? 'Off' : row.home ? 'Home' : row.in_use ? 'In use' : 'Not in use';
            const latency = !row.in_use || off ? '—' : row.latency_ms === null ? 'no reply yet' : `${row.latency_ms} ms`;
            return (
              <li key={row.url} className="flex flex-wrap items-center gap-x-s-3 gap-y-s-1 border-b border-border py-s-2">
                <span className="min-w-0 flex-1 break-all font-mono text-meta">{row.url}</span>
                <span className="text-meta text-muted-foreground">
                  {sourceText(row)}
                  {row.verified ? <span className="ml-s-2 text-text-strong">Verified</span> : null}
                </span>
                <span className="w-24 text-right text-meta tabular-nums">{latency}</span>
                <span className="w-16 text-meta text-text-strong">{state}</span>
                <Button
                  variant="outline"
                  size="sm"
                  aria-pressed={isPinned}
                  aria-label={`${isPinned ? 'Unpin' : 'Pin'} ${row.url}`}
                  disabled={!isPinned && (off || pinFull)}
                  title={off ? 'Turn it on before pinning it' : !isPinned && pinFull ? `At most ${PIN_LIMIT} relays can be pinned` : undefined}
                  onClick={() => onChange({ ...settings, pinned: toggle(settings.pinned, row.url) })}
                >
                  {isPinned ? 'Unpin' : 'Pin'}
                </Button>
                {row.source === 'community' ? (
                  <span className="w-24 text-right text-meta text-muted-foreground">Set by community</span>
                ) : (
                  <Button
                    variant="outline"
                    size="sm"
                    aria-pressed={off}
                    aria-label={`${off ? 'Turn on' : 'Turn off'} ${row.url}`}
                    disabled={isPinned && !off}
                    title={isPinned && !off ? 'Unpin it before turning it off' : undefined}
                    onClick={() => onChange({ ...settings, disabled: toggle(settings.disabled, row.url) })}
                  >
                    {off ? 'Turn on' : 'Turn off'}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <Hint>
        The node uses the fastest relay it can reach as home, and moves to a faster one when it measures one. Relays that do not answer stay listed
        and are not used. Relays a community names stay on this device until its owners or admins remove them.
      </Hint>
    </div>
  );
}

// The most relays a person may pin. The desktop settings check refuses more.
const PIN_LIMIT = 30;

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
