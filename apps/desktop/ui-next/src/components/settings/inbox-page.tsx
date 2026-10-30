// Settings > Offline delivery. An inbox is an always-on device that holds your direct messages while you are offline: your
// own seeder, another device of yours, or a friend's computer that agreed to. You pick inboxes from the devices that offer,
// and each one's status is asked live. Below, what this device holds for others. See docs/design/10-dm-delivery.md.
import { useCallback, useEffect, useState } from 'react';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api, onEngineEvent } from '@/lib/api';
import { formatSize } from '@/lib/attachments';
import { inboxNoticeText } from '@/lib/inbox-notices';
import { plural } from '@/lib/identity';
import { stampLabel } from '@/lib/time';
import { toast } from '@/lib/toasts';
import type { InboxSettings, InboxView, Person } from '@/lib/types';

const LIST = 'm-0 flex list-none flex-col gap-0.5 rounded-lg border border-border bg-panel p-s-2';
const ROW = 'flex min-h-14 items-center gap-s-3 rounded-lg px-s-2';

// One line on what the inbox holds for you, or why it holds nothing.
function statusLine(v: InboxView): string {
  if (!v.reachable) return 'Not answering right now. Messages wait on your contacts’ devices until it is back.';
  if (!v.serving && v.own_device) return 'Not holding your messages. Turn on “Hold my own messages” on that device.';
  if (!v.serving) return 'Not holding your messages. Its owner has to turn this on for you.';
  const held = v.envelopes ? `${plural(v.envelopes, 'message')} waiting · ${formatSize(v.bytes)}` : 'Nothing waiting';
  return `${held} · ${plural(v.key_packages, 'key')} for new conversations`;
}

function owner(v: InboxView): string {
  return v.own_device ? 'Your device' : v.name;
}

export function InboxPage() {
  const [chosen, setChosen] = useState<InboxView[] | null>(null);
  const [offers, setOffers] = useState<InboxView[] | null>(null);
  const [finding, setFinding] = useState(false);
  const [manual, setManual] = useState('');
  const [settings, setSettings] = useState<InboxSettings | null>(null);

  const reload = useCallback(() => {
    api
      .inboxStatus()
      .then(setChosen)
      .catch((err) => toast(err));
    api
      .inboxSettings()
      .then(setSettings)
      .catch((err) => toast(err));
  }, []);
  useEffect(reload, [reload]);
  // A notice that a device started or stopped holding your mail shows here at once.
  useEffect(() => {
    let off: (() => void) | null = null;
    let live = true;
    void onEngineEvent((ev) => {
      if (ev.kind === 'inbox_host_changed') reload();
    })
      .then((unlisten) => {
        if (live) off = unlisten;
        else unlisten();
      })
      .catch(() => {});
    return () => {
      live = false;
      off?.();
    };
  }, [reload]);

  const save = async (devices: string[]) => {
    try {
      await api.setInboxes(devices);
      setOffers((list) => list?.filter((o) => !devices.includes(o.device)) ?? null);
      reload();
    } catch (err) {
      toast(err);
    }
  };
  const ids = (chosen ?? []).map((v) => v.device);
  const add = (device: string) => void save([...ids, device]);
  const remove = (device: string) => void save(ids.filter((d) => d !== device));
  const find = async () => {
    setFinding(true);
    try {
      setOffers(await api.inboxCandidates());
    } catch (err) {
      toast(err);
    } finally {
      setFinding(false);
    }
  };
  const full = settings ? ids.length >= settings.max_inboxes : false;

  return (
    <div className="flex min-h-full max-w-[740px] flex-col gap-s-3">
      <h2 className="mb-s-2 text-head font-bold text-text-strong">Offline delivery</h2>
      <p className="text-muted-foreground">
        An inbox is an always-on device that holds your direct messages while you are offline, such as a seeder you run or a
        friend’s computer. It keeps them encrypted and cannot read them. Your contacts deliver there when you are away, and
        your devices collect from it when you come back.
      </p>

      {settings?.notices.length ? (
        <Block title="Notices">
          <ul aria-label="Inbox notices" className={LIST}>
            {settings.notices.map((n) => (
              <li key={n.device} className={ROW} data-inbox-notice={n.device} data-inbox-notice-on={n.on ? 'on' : 'off'}>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="text-body text-text-strong">{n.on ? `${n.name} now holds your messages while you are offline.` : inboxNoticeText(n.name, false)}</span>
                  <span className="text-meta text-muted-foreground">{stampLabel(n.ts_ms)}</span>
                </span>
                {n.on && !n.chosen && !ids.includes(n.device) ? (
                  <Button disabled={full} aria-label={`Use ${n.name} as an inbox`} onClick={() => add(n.device)}>
                    Use as inbox
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        </Block>
      ) : null}

      <Block title="Your inboxes">
        {chosen === null ? (
          <Hint>Asking your inboxes…</Hint>
        ) : chosen.length === 0 ? (
          <Hint data-inbox-empty>No inbox yet. Messages reach you only while one of your devices is online.</Hint>
        ) : (
          <ul aria-label="Your inboxes" className={LIST}>
            {chosen.map((v) => (
              <li key={v.device} className={ROW} data-inbox-row={v.device} data-inbox-state={!v.reachable ? 'unreachable' : v.serving ? 'serving' : 'refusing'}>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="text-body text-text-strong">{owner(v)}</span>
                  <span className="text-meta text-muted-foreground">{statusLine(v)}</span>
                </span>
                <Button variant="outline" aria-label={`Remove ${owner(v)} as an inbox`} onClick={() => remove(v.device)}>
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex justify-end">
          <Button variant="outline" onClick={reload}>
            Check again
          </Button>
        </div>
      </Block>

      <Block title="Add an inbox">
        <Hint>
          Devices that offer to hold your messages: your other devices with their inbox switched on, and friends and seeders
          that agreed to. Looking asks each of them, so they learn you looked.
        </Hint>
        <div className="flex">
          <Button onClick={() => void find()} disabled={finding || full}>
            {finding ? 'Looking…' : 'Find devices that offer'}
          </Button>
        </div>
        {full ? <Hint>You have {settings?.max_inboxes} inboxes, the most there can be. Remove one to add another.</Hint> : null}
        {offers === null ? null : offers.length === 0 ? (
          <Hint data-inbox-no-offers>No device offers yet. Ask a friend to turn on “Hold messages for” you, or start a seeder with --inbox-for and your address.</Hint>
        ) : (
          <ul aria-label="Devices that offer" className={LIST}>
            {offers.map((v) => (
              <li key={v.device} className={ROW} data-inbox-offer={v.device}>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="text-body text-text-strong">{owner(v)}</span>
                  <span className="text-meta text-muted-foreground">{statusLine(v)}</span>
                </span>
                <Button disabled={full} aria-label={`Use ${owner(v)} as an inbox`} onClick={() => add(v.device)}>
                  Use as inbox
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex items-center gap-s-2">
          <Input placeholder="Device id" aria-label="Inbox device id" value={manual} onChange={(e) => setManual(e.target.value)} />
          <Button
            variant="outline"
            disabled={full || !manual.trim()}
            onClick={() => {
              add(manual.trim());
              setManual('');
            }}
          >
            Add by id
          </Button>
        </div>
      </Block>

      <Block title="What an inbox learns">
        <ul className="m-0 flex list-disc flex-col gap-1 pl-5 text-sm leading-[19px] text-muted-foreground">
          <li>Who you are, and the ids of your devices.</li>
          <li>Who sent each message, how big it is and when, and when you come back to collect.</li>
          <li>The name and members of a group someone adds you to while you are away.</li>
          <li>Not what the messages say, or your contacts list.</li>
        </ul>
        <Hint>Choose people you trust with that. An inbox keeps each message for up to 30 days.</Hint>
      </Block>

      <HostBlock settings={settings} onChange={reload} />
    </div>
  );
}

// What this device holds for others, and the switch for your own messages on this device.
function HostBlock({ settings, onChange }: { settings: InboxSettings | null; onChange: () => void }) {
  const [contacts, setContacts] = useState<Person[]>([]);
  useEffect(() => {
    api
      .listContacts()
      .then((v) => setContacts(v.contacts))
      .catch(() => {});
  }, []);
  if (!settings) return null;
  const hosted = new Map(settings.hosted.map((h) => [h.root, h]));

  const serveOwn = async (on: boolean) => {
    try {
      await api.setServeInbox(on);
      onChange();
    } catch (err) {
      toast(err);
    }
  };
  const host = async (root: string, on: boolean) => {
    try {
      await api.hostInboxFor(root, on);
      onChange();
    } catch (err) {
      toast(err);
    }
  };

  // Hosted people who are not contacts (a seeder started with --inbox-for, say) are listed too.
  const people = [
    ...contacts.map((c) => ({ root: c.root_id, name: c.display_name })),
    ...settings.hosted.filter((h) => !contacts.some((c) => c.root_id === h.root)).map((h) => ({ root: h.root, name: h.name })),
  ];

  return (
    <Block title="Hold messages for others">
      <Hint>
        This device keeps messages for the people you tick while they are offline, and they can pick it as an inbox. Turn this
        on only on a computer that stays on. {settings.envelopes ? `It holds ${plural(settings.envelopes, 'message')} now, ${formatSize(settings.bytes)}.` : ''}
      </Hint>
      <CheckRow>
        <input
          type="checkbox"
          aria-label="Hold my own messages on this device"
          checked={settings.serving_own}
          onChange={(e) => void serveOwn(e.target.checked)}
          className={CHECK_CLASS}
          data-inbox-serve-own
        />
        <span>Hold my own messages on this device, for my other devices</span>
      </CheckRow>
      {people.length === 0 ? (
        <Hint>Add friends to hold messages for them.</Hint>
      ) : (
        people.map((p) => {
          const h = hosted.get(p.root);
          const atCap = !h && settings.hosted.length >= settings.max_hosted;
          return (
            <CheckRow key={p.root}>
              <input
                type="checkbox"
                aria-label={`Hold messages for ${p.name}`}
                checked={Boolean(h)}
                disabled={atCap}
                onChange={(e) => void host(p.root, e.target.checked)}
                className={CHECK_CLASS}
                data-inbox-host={p.root}
              />
              <span>
                Hold messages for {p.name}
                {h && h.envelopes ? <span className="text-muted-foreground"> · {plural(h.envelopes, 'message')} waiting</span> : null}
              </span>
            </CheckRow>
          );
        })
      )}
    </Block>
  );
}
