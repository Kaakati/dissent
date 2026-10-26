// Channel settings: a full-page overlay built like Settings. Overview holds the name, slowmode and content visibility,
// and voice channels add bitrate, user limit and relay. Permissions is not built. Invites lists the server's invites
// with Revoke. Changes wait in a save bar until Save. Owner and admin only. Ported from the old UI's views/channel-settings.js.
import { type ReactNode, useCallback, useEffect, useState } from 'react';
import { Hash, Trash2, Volume2, X } from 'lucide-react';
import { create } from 'zustand';
import { cleanChannelName } from '@/components/dialogs/create-channel-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { deleteChannel, loadInvites, updateChannel } from '@/lib/actions';
import { api } from '@/lib/api';
import { confirm, useDialogs } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { ChannelView, Id } from '@/lib/types';
import { cn } from '@/lib/utils';

const SLOWMODE: [string, number][] = [
  ['Off', 0], ['5s', 5], ['10s', 10], ['15s', 15], ['30s', 30], ['1m', 60], ['2m', 120],
  ['5m', 300], ['10m', 600], ['15m', 900], ['30m', 1800], ['1h', 3600], ['2h', 7200], ['6h', 21600],
];
const VISIBILITY: [Fields['visibility'], string, string][] = [
  ['default', 'Default', 'Messages in this channel are visible to everyone who can view it.'],
  ['spoiler', 'Spoiler Channel', 'Messages and attachments are blurred until someone clicks to reveal them.'],
  ['age', 'Age-Restricted Channel', 'People must confirm they are of age once before they can view this channel.'],
];
const BITRATE_HELP = 'Going above 64 kbps may adversely affect people on poor connections.';
const RELAY_HELP = 'Picks which relay helps people who can’t connect directly to each other in this channel.';
const SECTIONS = [
  { key: 'overview', label: 'Overview' },
  { key: 'permissions', label: 'Permissions' },
  { key: 'invites', label: 'Invites' },
] as const;
const SECTION_TITLE = { overview: 'Overview', permissions: 'Permissions', invites: 'Invites' } as const;

type Section = (typeof SECTIONS)[number]['key'];

interface Fields {
  name: string;
  slowmode_s: number;
  visibility: 'default' | 'spoiler' | 'age';
  bitrate_kbps: number;
  user_limit: number;
  relay: string; // 'automatic', or a relay's URL
}

const fromChannel = (ch: ChannelView): Fields => ({
  name: ch.name,
  slowmode_s: ch.slowmode_s ?? 0,
  visibility: ch.visibility ?? 'default',
  bitrate_kbps: ch.bitrate_kbps ?? 64,
  user_limit: ch.user_limit ?? 0,
  relay: ch.relay || 'automatic',
});

interface Target {
  communityId: Id;
  channelId: Id;
}

export const useChannelSettings = create<{ target: Target | null }>(() => ({ target: null }));

// Opens the overlay for a channel of the open server. Only owners and admins get it.
export function openChannelSettings(communityId: Id, channelId: Id): void {
  const s = useShell.getState();
  const role = s.community?.my_role;
  const channel = s.community?.id === communityId ? s.community.channels.find((c) => c.id === channelId) : undefined;
  if (!channel || (role !== 'owner' && role !== 'admin')) return;
  useChannelSettings.setState({ target: { communityId, channelId } });
}

export function ChannelSettingsHost() {
  const target = useChannelSettings((s) => s.target);
  return target ? <ChannelSettingsPage key={`${target.communityId}:${target.channelId}`} target={target} /> : null;
}

function ChannelSettingsPage({ target }: { target: Target }) {
  const { communityId, channelId } = target;
  const channel = useShell((s) => (s.community?.id === communityId ? s.community.channels.find((c) => c.id === channelId) : undefined));
  const relayUrls = useShell((s) => (s.community?.id === communityId ? (s.community.relays ?? []) : []));
  const [section, setSection] = useState<Section>('overview');
  // The saved values follow the store when the engine reports a change, and the draft follows them, so a save leaves
  // nothing to save. Edits are kept while the saved values stay the same.
  const saved = channel ? fromChannel(channel) : null;
  const [draft, setDraft] = useState<Fields | null>(saved);
  const [seen, setSeen] = useState<Fields | null>(saved);
  if (saved && JSON.stringify(saved) !== JSON.stringify(seen)) {
    setSeen(saved);
    setDraft(saved);
  }

  const changed = saved && draft ? (Object.keys(draft) as (keyof Fields)[]).filter((k) => draft[k] !== saved[k]) : [];
  const dirty = changed.length > 0;

  const close = useCallback(async () => {
    if (dirty && !(await confirm({ message: 'Discard your unsaved channel changes?', okLabel: 'Discard' }))) return;
    useChannelSettings.setState({ target: null });
  }, [dirty]);

  // Escape closes the overlay unless a dialog is open above it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      const dialogs = useDialogs.getState();
      if (dialogs.dialogs.length > 0 || dialogs.confirm) return;
      e.preventDefault();
      e.stopPropagation();
      void close();
    };
    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [close]);

  if (!channel || !saved || !draft) return null;
  const voice = Boolean(channel.voice);

  const save = async () => {
    const patch = Object.fromEntries(changed.map((k) => [k, draft[k]])) as Partial<Fields>;
    if (await updateChannel(communityId, channelId, patch)) toast('Channel saved', 'info');
  };
  const reset = () => setDraft(saved);
  const remove = async () => {
    const ok = await confirm({
      message: `Are you sure you want to delete ${voice ? '' : '#'}${saved.name}? Its messages will be lost for everyone.`,
      okLabel: 'Delete Channel',
    });
    if (!ok) return;
    if (await deleteChannel(communityId, channelId)) useChannelSettings.setState({ target: null });
  };
  const edit = (patch: Partial<Fields>) => setDraft((d) => (d ? { ...d, ...patch } : d));

  return (
    <div role="dialog" aria-modal="true" aria-label="Channel settings" className="fixed inset-x-0 top-[30px] bottom-0 z-[70] grid grid-cols-[560px_minmax(0,1fr)_auto] items-stretch bg-rail">
      <div className="flex min-h-0 justify-end overflow-y-auto bg-sidebar px-s-4 pt-[60px] pb-s-6">
        <nav aria-label="Channel settings sections" className="flex w-[232px] flex-col gap-0.5">
          <h2 className="mb-s-2 flex items-center gap-s-2 px-2.5 text-sm leading-5 font-semibold text-text-strong">
            {voice ? <Volume2 className="size-4 shrink-0 text-muted-foreground" /> : <Hash className="size-4 shrink-0 text-muted-foreground" />}
            <span className="truncate">{`${saved.name} · ${voice ? 'Voice' : 'Text'} Channels`}</span>
          </h2>
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              type="button"
              aria-current={s.key === section ? 'true' : undefined}
              onClick={() => setSection(s.key)}
              className={cn(
                'flex h-9 w-full items-center gap-s-2 rounded-lg px-2.5 text-left text-ui font-medium text-muted-foreground hover:bg-hover hover:text-text',
                s.key === section && 'bg-selected text-text-strong',
              )}
            >
              {s.label}
            </button>
          ))}
          <div className="my-s-2 h-px bg-divider" role="separator" />
          <button
            type="button"
            onClick={() => void remove()}
            className="flex h-9 w-full items-center gap-s-2 rounded-lg bg-danger px-2.5 text-left text-ui font-medium text-white hover:bg-danger-hover"
          >
            <Trash2 className="size-4" aria-hidden="true" />
            Delete Channel
          </button>
        </nav>
      </div>
      <div className="flex min-h-0 gap-s-8 overflow-y-auto bg-chat px-s-10 pt-[60px] pb-s-10">
        <div className="flex min-h-full w-[760px] max-w-full flex-col gap-7">
          <header>
            <h1 className="text-title font-bold text-text-strong">{SECTION_TITLE[section]}</h1>
          </header>
          {section === 'overview' ? <Overview voice={voice} draft={draft} relayUrls={relayUrls} edit={edit} /> : null}
          {section === 'permissions' ? (
            <Block title="Coming soon">
              <p className="text-sm text-muted-foreground">Per-channel roles are not built yet, so channel permissions cannot be set here.</p>
            </Block>
          ) : null}
          {section === 'invites' ? <InviteList communityId={communityId} /> : null}
          {dirty ? (
            <div role="region" aria-label="Unsaved changes" className="sticky bottom-0 mt-auto flex items-center gap-s-3 rounded-lg border border-border bg-elevated px-s-4 py-s-3">
              <span className="mr-auto text-ui font-medium text-text-strong">Careful — you have unsaved changes!</span>
              <Button variant="outline" onClick={reset}>Reset</Button>
              <Button onClick={() => void save()}>Save</Button>
            </div>
          ) : null}
        </div>
      </div>
      <div className="flex flex-col items-center gap-1.5 pt-[60px] pr-s-4">
        <button
          type="button"
          aria-label="Close channel settings"
          title="Close (Esc)"
          onClick={() => void close()}
          className="grid size-10 place-items-center rounded-full text-muted-foreground shadow-[inset_0_0_0_2px_var(--faint)] hover:bg-hover hover:text-text-strong"
        >
          <X className="size-[18px]" />
        </button>
        <span className="text-meta font-semibold text-muted-foreground">ESC</span>
      </div>
    </div>
  );
}

function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2.5">
      <h2 className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{title}</h2>
      {children}
    </section>
  );
}

function Overview({
  voice,
  draft,
  relayUrls,
  edit,
}: {
  voice: boolean;
  draft: Fields;
  relayUrls: string[];
  edit: (patch: Partial<Fields>) => void;
}) {
  const relays = [{ id: 'automatic', name: 'Automatic' }, ...relayUrls.map((url) => ({ id: url, name: safeHost(url) }))];
  return (
    <div className="flex flex-col gap-s-4">
      <Block title="Channel Name">
        <Input value={draft.name} maxLength={64} aria-label="Channel Name" onChange={(e) => edit({ name: cleanChannelName(e.target.value, voice) })} />
      </Block>
      {voice ? null : (
        <Block title="Slowmode">
          <select
            aria-label="Slowmode"
            value={String(draft.slowmode_s)}
            onChange={(e) => edit({ slowmode_s: Number(e.target.value) })}
            className="h-10 rounded-lg border border-input-border bg-input px-s-3 text-ui text-text-strong"
          >
            {SLOWMODE.map(([label, seconds]) => (
              <option key={seconds} value={String(seconds)}>{label}</option>
            ))}
          </select>
          <p className="text-sm text-muted-foreground">Members wait this long between messages. Owners and admins are not affected.</p>
        </Block>
      )}
      <fieldset className="flex flex-col gap-2.5">
        <legend className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">Content Visibility</legend>
        {VISIBILITY.map(([value, label, help]) => (
          <label key={value} className="flex cursor-pointer items-start gap-s-3 rounded-lg border border-border p-s-3 hover:bg-elevated">
            <input
              type="radio"
              name="channel-visibility"
              value={value}
              checked={draft.visibility === value}
              onChange={() => edit({ visibility: value })}
              className="mt-0.5 size-5 shrink-0 cursor-pointer appearance-none rounded-full border-2 border-faint checked:border-accent checked:bg-[radial-gradient(circle,var(--text-strong)_0_4px,var(--accent)_5px)]"
            />
            <span className="flex min-w-0 flex-col gap-1">
              <span className="text-body leading-[19px] font-semibold text-text-strong">{label}</span>
              <span className="text-sm leading-[17px] text-muted-foreground">{help}</span>
            </span>
          </label>
        ))}
      </fieldset>
      {voice ? (
        <>
          <Block title="Bitrate">
            <div className="flex items-center gap-s-3">
              <input
                type="range"
                min={8}
                max={96}
                step={1}
                value={draft.bitrate_kbps}
                aria-label="Bitrate"
                onChange={(e) => edit({ bitrate_kbps: Number(e.target.value) })}
                className="min-w-0 flex-1 accent-accent"
              />
              <output className="min-w-18 text-right text-sm text-text-strong">{`${draft.bitrate_kbps} kbps`}</output>
            </div>
            <p className="text-sm text-muted-foreground">{BITRATE_HELP}</p>
          </Block>
          <Block title="User Limit">
            <div className="flex items-center gap-s-3">
              <input
                type="range"
                min={0}
                max={99}
                step={1}
                value={draft.user_limit}
                aria-label="User Limit"
                onChange={(e) => edit({ user_limit: Number(e.target.value) })}
                className="min-w-0 flex-1 accent-accent"
              />
              <output className="min-w-18 text-right text-sm text-text-strong">{draft.user_limit === 0 ? '∞' : String(draft.user_limit)}</output>
            </div>
            <p className="text-sm text-muted-foreground">Most people who can join at once. ∞ means no limit.</p>
          </Block>
          <Block title="Relay">
            <select
              aria-label="Relay"
              value={draft.relay}
              onChange={(e) => edit({ relay: e.target.value })}
              className="h-10 rounded-lg border border-input-border bg-input px-s-3 text-ui text-text-strong"
            >
              {relays.map((r) => (
                <option key={r.id} value={r.id}>{r.name}</option>
              ))}
            </select>
            <p className="text-sm text-muted-foreground">{RELAY_HELP}</p>
          </Block>
        </>
      ) : null}
    </div>
  );
}

function safeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

// The server's invites, each with Revoke. Revoking reloads the list; a failure is a toast.
export function InviteList({ communityId }: { communityId: Id }) {
  const [items, setItems] = useState<{ id: Id; link: string }[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    loadInvites(communityId)
      .then((list) => {
        if (live) setItems(list as { id: Id; link: string }[]);
      })
      .catch((err: unknown) => {
        if (live) setError(`Invites are not available: ${String(err)}`);
      });
    return () => {
      live = false;
    };
  }, [communityId, tick]);

  const revoke = async (id: Id) => {
    try {
      await api.revokeInvite(communityId, id);
      setTick((n) => n + 1);
    } catch (err) {
      toast(err);
    }
  };

  return (
    <div className="flex flex-col gap-s-4">
      <Block title="Active invites">
        <p className="text-sm text-muted-foreground">
          Revoking a link stops new joins with it. Members already in the server stay. If a link has reached someone who must not read this server, ban them or rotate the server’s keys.
        </p>
        <div role="list" aria-label="Active invites" className="flex flex-col gap-s-2">
          {error ? <p className="text-sm text-muted-foreground">{error}</p> : null}
          {!error && items === null ? <p className="text-sm text-muted-foreground">Loading invites…</p> : null}
          {items && items.length === 0 ? <p className="text-sm text-muted-foreground">No active invites.</p> : null}
          {items?.map((inv) => (
            <div key={inv.id} role="listitem" className="flex items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3 py-s-2">
              <code title={inv.link} className="min-w-0 flex-1 truncate text-sm text-text">{inv.link}</code>
              <Button variant="outline" onClick={() => void revoke(inv.id)} aria-label={`Revoke invite ${inv.link}`}>Revoke</Button>
            </div>
          ))}
        </div>
      </Block>
      <div className="flex">
        <Button onClick={() => openInviteDialog(communityId)}>Create invite</Button>
      </div>
    </div>
  );
}
