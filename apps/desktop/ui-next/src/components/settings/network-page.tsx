// Settings > Network: the live status of the connection, the relays the node uses (where each came from, the latency measured to it, and the
// home relay), the relays you add, and the local network switch. Saving restarts the engine. The status and the relay list refresh every
// 15 seconds while the page is open. Ported from the network section of views/settings.js.
import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, X } from 'lucide-react';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api } from '@/lib/api';
import { confirm } from '@/lib/dialog';
import { formatDials } from '@/lib/dials';
import { toast } from '@/lib/toasts';
import { useShell } from '@/lib/store';
import { usePrivacyStatus } from '@/components/shell/privacy-indicator';
import type { NetStatus, NetworkSettings, PrivacyStatus, RelayRow, RelayStatus } from '@/lib/types';

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
        machine with a domain name and a valid certificate. The <a className="underline" href={HOSTING_GUIDE} rel="noreferrer">guide</a> covers the setup.
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

      <HideIpBlock key={`hide-${version}`} settings={settings} saving={saving || !loaded} onSave={async (next) => {
        setSaving(true);
        const saved = await setNetworkSettings(next);
        setSaving(false);
        if (saved) {
          setSettings(saved);
          setVersion((v) => v + 1);
        }
      }} />

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

// Hide my IP address, three ways. Off: direct connections, the fastest. Behind relays: peers see a relay's address, the relay
// sees yours. Through Tor: every connection also goes through the Tor you run, so the relays see a Tor exit; with Tor down
// nothing gets out and the app is offline, never direct. Each applies when the connection restarts, so it asks first and saves
// at once. While hidden, the status names the relay, and in Tor mode whether Tor is up.
type IpChoice = 'off' | 'relays' | 'tor';

const CHOICES: { value: IpChoice; label: string; hint: string }[] = [
  { value: 'off', label: 'Off', hint: 'Direct connections, the fastest. The people you talk to can see your IP address.' },
  {
    value: 'relays',
    label: 'Hide behind relays',
    hint: 'Everything goes through a relay, so the people you talk to never see your IP address. The relay sees it, and when and with whom you connect, but not what you send.',
  },
  {
    value: 'tor',
    label: 'Hide through Tor',
    hint: 'Everything also goes through Tor: the one built into Dissent, or your own. The relays see a Tor exit, not you. Slow: voice may lag or drop, and video isn’t practical. If Tor stops, Dissent goes offline rather than connect directly.',
  },
];

function choiceOf(settings: NetworkSettings): IpChoice {
  if (!settings.hide_ip) return 'off';
  return settings.tor ? 'tor' : 'relays';
}

// Which Tor carries everything in Tor mode.
type TorChoice = 'builtin' | 'own';

const TOR_CHOICES: { value: TorChoice; label: string; hint: string }[] = [
  { value: 'builtin', label: 'Built into Dissent', hint: 'Nothing to install. It starts with Dissent and takes a few seconds to connect the first time.' },
  { value: 'own', label: 'My own Tor', hint: 'Tor, or Tor Browser while it is open, running on this computer.' },
];

// Whether these settings use the built-in Tor. A file from before it with Tor on uses its own; the app fills it in.
function torChoiceOf(settings: NetworkSettings): TorChoice {
  return (settings.tor_builtin ?? !settings.tor) ? 'builtin' : 'own';
}

const TOR_INTRO: Record<TorChoice, (socks: string) => string> = {
  builtin: () => 'Dissent starts its built-in Tor, which takes a few seconds to connect the first time.',
  own: (socks) => `Tor must be running on this computer at ${socks}.`,
};

const CONFIRM: Record<IpChoice, (socks: string, tor: TorChoice) => string> = {
  off: () => 'Show your IP address again? Direct connections come back, which are faster. Your connection restarts now, which ends a call you are in.',
  relays: () =>
    'Hide your IP address behind relays? Everything will go through a relay. Calls and file transfers get slower, and the relay sees your IP address and when and with whom you connect, but not what you send. Your connection restarts now, which ends a call you are in.',
  tor: (socks, tor) =>
    `Hide your IP address through Tor? ${TOR_INTRO[tor](socks)} Everything goes through it, so it is slow: voice may lag and video isn’t practical. If Tor stops, Dissent goes offline instead of connecting directly. Your connection restarts now, which ends a call you are in.`,
};

const CONFIRM_LABEL: Record<IpChoice, string> = { off: 'Show my IP address', relays: 'Hide behind relays', tor: 'Hide through Tor' };

function hideIpStatus(choice: IpChoice, status: PrivacyStatus | null, home: string | null, socks: string): string {
  if (choice === 'relays') {
    return home ? `Connected through relay ${home}. Peers can’t see your IP address; the relay can.` : 'Connecting through a relay…';
  }
  if (status?.tor_obsolete) {
    return 'Tor stopped: update Dissent. The Tor network no longer accepts this version of its built-in Tor. Nothing is sent until you update.';
  }
  if (status?.tor_ok === false && status.tor_builtin) {
    return `${status.tor_error ?? 'The built-in Tor can’t connect yet.'} Nothing is sent until it can.`;
  }
  if (status?.tor_ok === false) {
    return `Can’t reach Tor at ${status.tor_socks ?? socks}: ${status.tor_error ?? 'no answer'}. Nothing is sent until it can.`;
  }
  if (status?.tor_ok) {
    return `Connected through Tor${home ? ` and relay ${home}` : ''}. The relay sees a Tor exit, not your IP address.`;
  }
  return 'Connecting to Tor…';
}

function HideIpBlock({ settings, saving, onSave }: { settings: NetworkSettings; saving: boolean; onSave: (next: NetworkSettings) => Promise<void> }) {
  const choice = choiceOf(settings);
  const [socks, setSocks] = useState(settings.tor_socks || '127.0.0.1:9050');
  const [home, setHome] = useState<string | null>(null);
  const status = usePrivacyStatus(choice === 'tor');
  useEffect(() => {
    if (choice === 'off') return;
    let live = true;
    const read = () => api.relayStatus().then((r) => live && setHome(r.home ?? null), () => {});
    void read();
    const timer = setInterval(read, 5000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [choice]);
  const tor = torChoiceOf(settings);
  const change = async (next: IpChoice, nextSocks = socks, nextTor = tor) => {
    const ok = await confirm({ message: CONFIRM[next](nextSocks, nextTor), okLabel: CONFIRM_LABEL[next] });
    if (ok) await onSave({ ...settings, hide_ip: next !== 'off', tor: next === 'tor', tor_builtin: nextTor === 'builtin', tor_socks: nextSocks });
  };
  return (
    <Block title="Privacy">
      <fieldset className="m-0 flex flex-col gap-s-2 border-0 p-0" aria-label="Hide my IP address" disabled={saving}>
        <legend className="mb-s-1 text-body text-text-strong">Hide my IP address</legend>
        {CHOICES.map((c) => (
          <label key={c.value} className="flex items-start gap-s-2">
            <input
              type="radio"
              name="hide-ip"
              value={c.value}
              checked={choice === c.value}
              onChange={() => void change(c.value)}
              className="m-0 mt-0.5 size-5 shrink-0 accent-accent"
            />
            <span className="flex flex-col">
              <span>{c.label}</span>
              <Hint>{c.hint}</Hint>
            </span>
          </label>
        ))}
      </fieldset>
      {choice === 'tor' ? (
        <fieldset className="m-0 flex flex-col gap-s-2 border-0 p-0 pl-s-6" aria-label="Which Tor" disabled={saving}>
          <legend className="mb-s-1 text-body text-text-strong">Which Tor</legend>
          {TOR_CHOICES.map((c) => (
            <label key={c.value} className="flex items-start gap-s-2">
              <input
                type="radio"
                name="which-tor"
                value={c.value}
                checked={tor === c.value}
                onChange={() => void change('tor', socks.trim(), c.value)}
                className="m-0 mt-0.5 size-5 shrink-0 accent-accent"
              />
              <span className="flex flex-col">
                <span>{c.label}</span>
                <Hint>{c.hint}</Hint>
              </span>
            </label>
          ))}
          {tor === 'own' ? (
            <>
              <div className="flex items-center gap-s-2">
                <Input aria-label="Tor SOCKS port" value={socks} onChange={(e) => setSocks(e.target.value)} className="max-w-[220px] font-mono" />
                <Button variant="outline" disabled={saving || socks.trim() === (settings.tor_socks || '')} onClick={() => void change('tor', socks.trim())}>
                  Use this port
                </Button>
              </div>
              <Hint>Tor listens on 127.0.0.1:9050; Tor Browser on 127.0.0.1:9150, while it is open.</Hint>
            </>
          ) : null}
        </fieldset>
      ) : null}
      {choice !== 'off' ? (
        <Hint role="status" data-hide-ip-status>
          {hideIpStatus(choice, status, home, socks)}
        </Hint>
      ) : null}
    </Block>
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
