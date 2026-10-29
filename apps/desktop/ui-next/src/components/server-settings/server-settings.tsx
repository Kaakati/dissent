// Server Settings: a full-page overlay built like Settings, opened from the server menu by the owner. The nav lists what
// Dissent has: Server Profile, then People (Members, Invites, Bans). Server Profile is a form beside a live preview card,
// the same card an invite shows. The name, banner, description and traits wait in an unsaved bar until Save Changes,
// which publishes only what changed; the icon applies at once, through the server icon flow. Leaving with unsaved
// changes shakes the bar instead of closing, as Discord does.
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { Radio, Smile, X } from 'lucide-react';
import { create } from 'zustand';
import { BannerPicker } from '@/components/banner-picker';
import { InviteList } from '@/components/channel-settings/channel-settings';
import { openEmojiPicker } from '@/components/chat/emoji-picker';
import { ServerCard } from '@/components/server-card';
import { chooseImage } from '@/components/settings/avatar-editor';
import { BLOCK_TITLE, Block, CHECK_CLASS, CheckRow, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { api } from '@/lib/api';
import { clearCommunityIcon, moderate, saveServerProfile, setCommunityIcon, setCommunityIconAnimated, type ServerProfileChanges } from '@/lib/actions';
import { bannerOf, bannersStill, draftUrlOf, sameBanner, subscribeBannerMotion, type BannerDraft } from '@/lib/banners';
import { boostLabel, videoLabel } from '@/lib/boosts';
import { confirm, useDialogs } from '@/lib/dialog';
import { iconSrc } from '@/lib/images';
import { useShell } from '@/lib/store';
import { loadStorageAsks, useStorageAsks } from '@/lib/storage-asks';
import { toast } from '@/lib/toasts';
import type { CommunityView, Id, Trait } from '@/lib/types';
import { cn } from '@/lib/utils';

const MAX_NAME = 64; // the engine's limits, in characters
const MAX_DESCRIPTION = 300;
const MAX_LABEL = 24;
const SLOTS = 5;

export type Section = 'profile' | 'boosts' | 'members' | 'invites' | 'bans';
const NAV: { key: Section; label: string; group: 'server' | 'people' }[] = [
  { key: 'profile', label: 'Server Profile', group: 'server' },
  { key: 'boosts', label: 'Boosts', group: 'server' },
  { key: 'members', label: 'Members', group: 'people' },
  { key: 'invites', label: 'Invites', group: 'people' },
  { key: 'bans', label: 'Bans', group: 'people' },
];

type Slot = { emoji: string; label: string };

interface Draft {
  name: string;
  banner: BannerDraft;
  description: string;
  slots: Slot[];
}

const slotsOf = (traits: Trait[] | undefined): Slot[] =>
  Array.from({ length: SLOTS }, (_, i) => ({ emoji: traits?.[i]?.emoji ?? '', label: traits?.[i]?.label ?? '' }));

const draftOf = (c: CommunityView): Draft => ({
  name: c.name,
  banner: bannerOf(c.banner, c.banner_preset, c.banner_poster),
  description: c.description ?? '',
  slots: slotsOf(c.traits),
});

// The traits a draft names: the slots with a label, in order. The emoji is optional, as in Discord.
const traitsOf = (slots: Slot[]): Trait[] => slots.filter((s) => s.label.trim()).map((s) => ({ emoji: s.emoji, label: s.label.trim() }));
// A slot with an emoji and no label is not a trait yet, and Save says so.
const unlabelled = (s: Slot) => !!s.emoji && !s.label.trim();
const sameTraits = (a: Trait[], b: Trait[]) => JSON.stringify(a) === JSON.stringify(b);

export const useServerSettings = create<{ communityId: Id | null; section: Section }>(() => ({ communityId: null, section: 'profile' }));

// The owner and admins get the page, for the open server, at a section (the profile by default).
const canManage = (c: CommunityView | null | undefined): boolean => c?.my_role === 'owner' || c?.my_role === 'admin';
export function openServerSettings(communityId: Id, section: Section = 'profile'): void {
  const community = useShell.getState().community;
  if (community?.id !== communityId || !canManage(community)) return;
  useServerSettings.setState({ communityId, section });
}

export function ServerSettingsHost() {
  const communityId = useServerSettings((s) => s.communityId);
  const section = useServerSettings((s) => s.section);
  const community = useShell((s) => (s.community?.id === communityId ? s.community : null));
  // Leaving the server, or losing the role, closes the page.
  useEffect(() => {
    if (communityId && !canManage(community)) useServerSettings.setState({ communityId: null });
  }, [communityId, community]);
  return community && canManage(community) ? <ServerSettingsPage key={`${community.id}:${section}`} community={community} section={section} /> : null;
}

function ServerSettingsPage({ community, section: initial }: { community: CommunityView; section: Section }) {
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const [section, setSection] = useState<Section>(initial);
  // The saved values follow the store when the engine reports a change, and the draft follows them, so a save leaves
  // nothing to save. Edits are kept while the saved values stay the same.
  const saved = draftOf(community);
  const savedKey = JSON.stringify(saved);
  const [seen, setSeen] = useState(savedKey);
  const [draft, setDraft] = useState<Draft>(saved);
  if (savedKey !== seen) {
    setSeen(savedKey);
    setDraft(saved);
  }
  const [shake, setShake] = useState(0);
  const [saving, setSaving] = useState(false);

  const changes: ServerProfileChanges = {};
  if (draft.name.trim() !== saved.name) changes.name = draft.name.trim();
  if (!sameBanner(draft.banner, saved.banner)) {
    const b = draft.banner;
    changes.banner =
      b === null
        ? null
        : b.kind === 'preset'
          ? { kind: 'preset', id: b.id }
          : b.posterBytes
            ? { kind: 'gif', gif: b.bytes ?? '', poster: b.posterBytes }
            : { kind: 'image', bytes: b.bytes ?? '' };
  }
  const traits = traitsOf(draft.slots);
  if (draft.description.trim() !== saved.description || !sameTraits(traits, traitsOf(saved.slots)) || draft.slots.some(unlabelled)) {
    changes.profile = { description: draft.description.trim(), traits };
  }
  const dirty = Object.keys(changes).length > 0;

  const close = useCallback(() => {
    if (dirty) {
      setShake((n) => n + 1);
      return;
    }
    useServerSettings.setState({ communityId: null });
  }, [dirty]);

  // Escape closes the overlay unless a dialog is open above it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      const dialogs = useDialogs.getState();
      if (dialogs.dialogs.length > 0 || dialogs.confirm) return;
      e.preventDefault();
      e.stopPropagation();
      close();
    };
    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [close]);

  const save = async () => {
    if (changes.name === '') {
      toast('A server name cannot be empty');
      return;
    }
    if (draft.slots.some(unlabelled)) {
      toast('Give each trait with an emoji a label, or clear its emoji');
      return;
    }
    setSaving(true);
    const ok = await saveServerProfile(community.id, changes);
    setSaving(false);
    if (ok) toast('Server profile saved', 'info');
  };
  const edit = (patch: Partial<Draft>) => setDraft((d) => ({ ...d, ...patch }));

  const members = community.members.filter((m) => !m.banned);
  const asks = useStorageAsks((s) => s.asks[community.id]?.length ?? 0);
  useEffect(() => {
    void loadStorageAsks(community.id);
  }, [community.id]);
  const online = members.filter((m) => (m.status ?? (m.online ? 'online' : 'offline')) !== 'offline').length;

  return (
    <div role="dialog" aria-modal="true" aria-label="Server settings" data-server-settings className="fixed inset-x-0 top-[30px] bottom-0 z-[70] grid grid-cols-[minmax(240px,1fr)_minmax(0,3fr)_auto] items-stretch bg-rail">
      <div className="flex min-h-0 justify-end overflow-y-auto bg-sidebar px-s-4 pt-[60px] pb-s-6">
        <nav aria-label="Server settings sections" className="flex w-[232px] flex-col gap-0.5">
          {(['server', 'people'] as const).map((group) => (
            <div key={group} className="flex flex-col gap-0.5">
              <h2 className={cn('flex h-8 items-center truncate px-2.5', BLOCK_TITLE, group === 'people' && 'mt-s-3')} title={group === 'server' ? community.name : undefined}>
                {group === 'server' ? community.name : 'People'}
              </h2>
              {NAV.filter((n) => n.group === group).map((n) => (
                <button
                  key={n.key}
                  type="button"
                  aria-current={n.key === section ? 'true' : undefined}
                  onClick={() => setSection(n.key)}
                  className={cn(
                    'flex h-9 w-full items-center gap-s-2 rounded-lg px-2.5 text-left text-ui font-medium text-muted-foreground hover:bg-hover hover:text-text',
                    n.key === section && 'bg-selected text-text-strong',
                  )}
                >
                  {n.label}
                  {n.key === 'members' && asks ? (
                    <span data-asks-badge aria-label={`${asks} seeder${asks === 1 ? '' : 's'} asking for a grant`} className="ml-auto grid min-w-5 place-items-center rounded-full bg-danger px-1.5 text-meta font-semibold text-white">
                      {asks}
                    </span>
                  ) : null}
                </button>
              ))}
            </div>
          ))}
        </nav>
      </div>

      <div className="flex min-h-0 overflow-y-auto bg-chat px-s-10 pt-[60px] pb-s-10">
        <div className="flex min-h-full w-[900px] max-w-full flex-col gap-7">
          <header>
            <h1 className="text-title font-bold text-text-strong">{NAV.find((n) => n.key === section)?.label}</h1>
            {section === 'profile' ? <Hint className="mt-1">Customize how your server appears in invite links and to members.</Hint> : null}
          </header>
          {section === 'profile' ? (
            <div className="flex items-start gap-s-8">
              <ProfileForm community={community} draft={draft} edit={edit} />
              <aside aria-label="Preview" className="sticky top-0 w-[300px] shrink-0">
                <ServerCard
                  id={community.id}
                  name={draft.name.trim()}
                  icon={iconSrc(community.id)}
                  banner={draft.banner}
                  memberCount={members.length}
                  onlineCount={online}
                  createdMs={community.created_ms}
                  description={draft.description.trim()}
                  traits={traitsOf(draft.slots)}
                  boost={{ level: community.boost?.level ?? 0, hosts: community.boost?.hosts.length ?? 0 }}
                />
              </aside>
            </div>
          ) : null}
          {section === 'members' ? (
            <>
              <PeopleList people={members.filter((m) => !m.storage)} empty="No members yet." />
              <SeederList community={community} seeders={members.filter((m) => m.storage)} />
            </>
          ) : null}
          {section === 'invites' ? <InviteList communityId={community.id} /> : null}
          {section === 'bans' ? <BansList community={community} /> : null}
          {section === 'boosts' ? <BoostsSection community={community} /> : null}
          {dirty ? (
            <div
              key={shake}
              role="region"
              aria-label="Unsaved changes"
              data-shake={shake || undefined}
              className={cn('sticky bottom-0 mt-auto flex items-center gap-s-3 rounded-lg border border-border bg-elevated px-s-4 py-s-3', shake > 0 && 'animate-[unsaved-shake_0.45s_ease-in-out] border-danger')}
            >
              <span className="mr-auto text-ui font-medium text-text-strong">Careful — you have unsaved changes!</span>
              <Button variant="ghost" onClick={() => setDraft(saved)}>Reset</Button>
              <Button disabled={saving} className="bg-[var(--success-button)] text-white hover:bg-[var(--success-button)] hover:brightness-95" onClick={() => void save()}>
                Save Changes
              </Button>
            </div>
          ) : null}
        </div>
      </div>

      <div className="flex flex-col items-center gap-1.5 pt-[60px] pr-s-4">
        <button
          type="button"
          aria-label="Close server settings"
          title="Close (Esc)"
          onClick={close}
          className="grid size-10 place-items-center rounded-full text-muted-foreground shadow-[inset_0_0_0_2px_var(--faint)] hover:bg-hover hover:text-text-strong"
        >
          <X className="size-[18px]" />
        </button>
        <span className="text-meta font-semibold text-muted-foreground">ESC</span>
      </div>
    </div>
  );
}

function ProfileForm({ community, draft, edit }: { community: CommunityView; draft: Draft; edit: (patch: Partial<Draft>) => void }) {
  useShell((s) => s.communities); // the icon redraws once it has been saved and the list reloaded
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const icon = iconSrc(community.id);
  const emojiRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const setSlot = (i: number, patch: Partial<Slot>) => edit({ slots: draft.slots.map((s, j) => (j === i ? { ...s, ...patch } : s)) });

  return (
    <div className="flex min-w-0 flex-1 flex-col gap-7">
      <Block title="Name">
        <Input value={draft.name} maxLength={MAX_NAME} aria-label="Server Name" onChange={(e) => edit({ name: e.target.value })} />
      </Block>

      <Block title="Server Icon">
        <Hint>We recommend an image of at least 512×512.</Hint>
        <div className="flex items-center gap-s-3">
          <Button
            onClick={() =>
              chooseImage({
                shape: 'rounded',
                animated: true,
                onApply: (bytes, animated) =>
                  animated ? setCommunityIconAnimated(community.id, bytes, animated.poster) : setCommunityIcon(community.id, bytes),
              })
            }
          >
            Change Server Icon
          </Button>
          {icon ? (
            <button type="button" className="text-ui font-medium text-danger-text hover:underline" onClick={() => void clearCommunityIcon(community.id)}>
              Remove Icon
            </button>
          ) : null}
        </div>
      </Block>

      <Block title="Banner">
        <BannerPicker
          value={draft.banner}
          onPreset={(id) => edit({ banner: { kind: 'preset', id } })}
          onUpload={(bytes, animated) =>
            edit({
              banner: animated
                ? { kind: 'image', bytes, url: draftUrlOf(bytes), poster: draftUrlOf(animated.poster, 'poster'), posterBytes: animated.poster }
                : { kind: 'image', bytes, url: draftUrlOf(bytes) },
            })
          }
          onRemove={() => edit({ banner: null })}
        />
      </Block>

      <Block title="Traits">
        <Hint>Add up to 5 traits to show off your server’s interests and personality. The emoji is optional.</Hint>
        <div className="grid grid-cols-2 gap-s-2" data-trait-slots>
          {draft.slots.map((slot, i) => (
            <div
              key={i}
              data-trait-slot={i}
              aria-invalid={unlabelled(slot) || undefined}
              className={cn('flex h-10 items-center gap-s-2 rounded-lg border bg-input pl-1 pr-s-2', unlabelled(slot) ? 'border-danger' : 'border-input-border')}
            >
              <button
                ref={(el) => {
                  emojiRefs.current[i] = el;
                }}
                type="button"
                aria-label={slot.emoji ? `Trait ${i + 1} emoji ${slot.emoji}; right-click to clear` : `Pick an emoji for trait ${i + 1} (optional)`}
                onClick={() => {
                  const anchor = emojiRefs.current[i];
                  if (anchor) openEmojiPicker(anchor, (emoji) => setSlot(i, { emoji }), 'Trait emoji');
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setSlot(i, { emoji: '' });
                }}
                className="grid size-8 shrink-0 place-items-center rounded-md text-lg hover:bg-hover"
              >
                {slot.emoji || <Smile className="size-5 text-muted-foreground" aria-hidden="true" />}
              </button>
              <input
                value={slot.label}
                maxLength={MAX_LABEL}
                aria-label={`Trait ${i + 1} label`}
                placeholder="Trait"
                onChange={(e) => setSlot(i, { label: e.target.value })}
                className="h-full min-w-0 flex-1 bg-transparent text-ui text-text-strong placeholder:text-muted-foreground focus-visible:outline-none"
              />
            </div>
          ))}
        </div>
      </Block>

      <HistoryBlock community={community} />
      <Block title="Description">
        <Hint>How did your server get started? Why should people join?</Hint>
        <div className="relative">
          <Textarea
            value={draft.description}
            maxLength={MAX_DESCRIPTION}
            rows={4}
            aria-label="Description"
            onChange={(e) => edit({ description: e.target.value })}
            className="pb-6"
          />
          <span data-description-count className="pointer-events-none absolute right-s-3 bottom-2 text-xs text-muted-foreground tabular-nums">
            {`${[...draft.description].length}/${MAX_DESCRIPTION}`}
          </span>
        </div>
      </Block>
    </div>
  );
}

// Whether a new member can read the messages from before it joined. Applies at once, as the address switch does. "All"
// says what it costs, since a member who was here then can open a past message from any later point in time.
function HistoryBlock({ community }: { community: CommunityView }) {
  const change = async (setting: string) => {
    try {
      await api.setHistoryVisibility(community.id, setting);
      await useShell.getState().loadCommunity(community.id);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Message history">
      <label className="flex items-center gap-s-2 text-sm text-text">
        <span className="shrink-0">New members can read</span>
        <select
          aria-label="New members can read"
          value={community.history ?? 'since_join'}
          onChange={(e) => void change(e.target.value)}
          className="h-8 rounded-md bg-input px-s-2 text-sm text-text"
        >
          <option value="since_join">Messages after they joined</option>
          <option value="all">All past messages</option>
        </select>
      </label>
      {community.history === 'all' ? (
        <Hint>New members can read past messages. Messages are protected per encryption period rather than per message.</Hint>
      ) : null}
    </Block>
  );
}

// Boosts: the level this device verifies, the members who host relays for the server, what the next level needs,
// and the ceilings it gives calls. Anyone may add their own relay here. Nothing basic depends on a level.
function BoostsSection({ community }: { community: CommunityView }) {
  const boost = community.boost;
  const me = useShell((s) => s.me);
  const [relay, setRelay] = useState('');
  if (!boost) return <Hint>Boosts are not available on this server yet.</Hint>;
  const mine = boost.hosts.find((h) => h.root_id === me?.root_id);
  const toggleMembers = async (on: boolean) => {
    try {
      await api.setMembersHostRelays(community.id, on);
      await useShell.getState().loadCommunity(community.id);
    } catch (err) {
      toast(err);
    }
  };
  const publish = async (relays: string[]) => {
    try {
      await api.setHostedRelays(community.id, relays);
      await useShell.getState().loadCommunity(community.id);
      toast(relays.length ? 'Relay published. Members count it once their apps verify it.' : 'Relay withdrawn', 'info');
      setRelay('');
    } catch (err) {
      toast(err);
    }
  };
  return (
    <div className="flex max-w-[640px] flex-col gap-7">
      <Block title="Level">
        <p data-boost-summary className="m-0 text-body font-semibold text-text-strong">{boostLabel(boost.level, boost.hosts.length)}</p>
        <Hint>
          {boost.next_level_hosts !== null
            ? `${boost.next_level_hosts - boost.hosts.length} more verified relay host${boost.next_level_hosts - boost.hosts.length === 1 ? '' : 's'} reach level ${boost.level + 1}.`
            : 'This is the top level.'}{' '}
          {`Calls here can use up to ${boost.voice_kbps} kbps voice and ${videoLabel(boost.video_height, boost.video_fps)} video, as a mesh of up to ${boost.mesh_limit}.`}
        </Hint>
        <Hint>Boosts are never paid for and never lock anything: chat, files, calls and screen sharing work the same at level 0.</Hint>
      </Block>
      <Block title="Relay hosts">
        {boost.hosts.length ? (
          <>
            <p className="m-0 text-sm text-text">Thank you to the members who keep this server fast by hosting relays.</p>
            <ul data-boost-hosts className="m-0 flex list-none flex-col gap-s-1 p-0">
              {boost.hosts.map((h) => (
                <li key={h.root_id} className="flex min-h-11 items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3 py-s-2">
                  <Radio className="size-4 shrink-0 text-link" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{h.display_name}</span>
                  <span className="min-w-0 truncate text-sm text-muted-foreground">{h.relays.join(', ')}</span>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <Hint>No member hosts a verified relay for this server yet.</Hint>
        )}
      </Block>
      {community.my_role === 'owner' || community.my_role === 'admin' ? (
        <Block title="Who can host">
          <CheckRow>
            <input
              type="checkbox"
              className={CHECK_CLASS}
              aria-label="Members can host relays"
              checked={Boolean(boost.members_can_host)}
              onChange={(e) => void toggleMembers(e.target.checked)}
            />
            Members can host relays
          </CheckRow>
          <Hint>Off: only the owner and admins can add a relay here. On: any member can, and the apps still check each one.</Hint>
        </Block>
      ) : null}
      <SeederForwardsBlock community={community} />
      <Block title="Host a relay">
        <Hint>Run the Dissent relay (see the relay guide), then add its address here. Each member’s app checks it before it counts.</Hint>
        <div className="flex gap-s-2">
          <Input value={relay} aria-label="Relay address" placeholder="https://relay.example.org" disabled={boost.can_publish === false} onChange={(e) => setRelay(e.target.value)} />
          <Button disabled={!relay.trim() || boost.can_publish === false} onClick={() => void publish([relay.trim()])}>Publish</Button>
        </div>
        {boost.can_publish === false ? <Hint>Only the owner and admins can add relays on this server.</Hint> : null}
        {mine ? (
          <button type="button" className="self-start text-ui font-medium text-danger-text hover:underline" onClick={() => void publish([])}>
            Withdraw my relay
          </button>
        ) : null}
      </Block>
    </div>
  );
}

// Seeder may forward calls: the owner lets the server's seeders forward its voice calls when nobody in a call
// forwards better, as members who turn on Help Forward Calls do. Off by default. Admins see it, but only the owner
// changes it, which the engine enforces too.
function SeederForwardsBlock({ community }: { community: CommunityView }) {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    let live = true;
    api.getSeederForwardsCalls(community.id).then((v) => live && setOn(v), () => live && setOn(null));
    return () => {
      live = false;
    };
  }, [community.id]);
  if (on === null) return null;
  const owner = community.my_role === 'owner';
  const change = async (next: boolean) => {
    try {
      await api.setSeederForwardsCalls(community.id, next);
      setOn(next);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Calls">
      <CheckRow>
        <input
          type="checkbox"
          className={CHECK_CLASS}
          aria-label="Seeder may forward calls"
          checked={on}
          disabled={!owner}
          onChange={(e) => void change(e.target.checked)}
        />
        Seeder may forward calls
      </CheckRow>
      <Hint>
        On: this server’s seeder can pass a voice call’s sealed streams along when it is the best-connected device, and the
        call shows its name. It can’t open them, but it learns who is in the call and who speaks.
      </Hint>
      {owner ? null : <Hint>Only the owner can change this.</Hint>}
    </Block>
  );
}

// The server's seeders, storage members outside the encrypted group, and whether an admin granted each one storage.
// A seeder asks the owner and admins for a grant when it joins. Grants are not required yet; they will be, once seeders
// have had time to ask (docs/design/05-encrypted-servers.md, "Storage joins").
function SeederList({ community, seeders }: { community: CommunityView; seeders: CommunityView['members'] }) {
  const asks = useStorageAsks((s) => s.asks[community.id] ?? []);
  const key = seeders.map((m) => `${m.root_id}:${m.storage_granted ? 1 : 0}`).join(',');
  useEffect(() => {
    void loadStorageAsks(community.id);
  }, [community.id, key]);
  if (!seeders.length) return null;
  const grant = async (rootId: string, granted: boolean) => {
    try {
      await api.grantStorage(community.id, rootId, granted);
      await useShell.getState().loadCommunity(community.id);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Seeders">
      <Hint>A seeder keeps the server’s history available. It can’t read messages. Grant the seeders you run or trust.</Hint>
      <ul role="list" aria-label="Seeders" className="m-0 flex list-none flex-col gap-s-1 p-0">
        {seeders.map((m) => (
          <li key={m.root_id} data-seeder={m.root_id} className="flex h-11 items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3">
            <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{m.display_name}</span>
            <span data-seeder-grant className="text-sm text-muted-foreground">
              {m.storage_granted ? 'Granted' : asks.includes(m.root_id) ? 'Asks for a grant' : 'Not granted'}
            </span>
            <Button variant={m.storage_granted ? 'ghost' : 'default'} onClick={() => void grant(m.root_id, !m.storage_granted)}>
              {m.storage_granted ? 'Withdraw grant' : 'Grant storage'}
            </Button>
          </li>
        ))}
      </ul>
    </Block>
  );
}

// Who may lift a ban, as replay decides: the owner for anyone, an admin for anyone who was not an admin. The page is
// for the owner and admins only.
function canUnban(mine: CommunityView['my_role'], target: string): boolean {
  return mine === 'owner' || (mine === 'admin' && target !== 'admin' && target !== 'owner');
}

// The banned members, each with Unban after a confirm.
function BansList({ community }: { community: CommunityView }) {
  const banned = community.members.filter((m) => m.banned);
  if (!banned.length) return <Hint>Nobody is banned.</Hint>;
  const unban = async (m: CommunityView['members'][number]) => {
    if (await confirm({ message: `Unban ${m.display_name}? They can rejoin ${community.name} with an invite link.`, okLabel: 'Unban' })) {
      await moderate(community.id, 'unban', m.root_id);
    }
  };
  return (
    <ul role="list" className="m-0 flex list-none flex-col gap-s-1 p-0" data-bans>
      {banned.map((m) => (
        <li key={m.root_id} className="flex h-11 items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3">
          <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{m.display_name}</span>
          <span className="text-sm text-muted-foreground capitalize">{m.role}</span>
          {canUnban(community.my_role, m.role) ? (
            <Button size="sm" variant="secondary" onClick={() => void unban(m)} aria-label={`Unban ${m.display_name}`}>
              Unban
            </Button>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

function PeopleList({ people, empty }: { people: CommunityView['members']; empty: string }) {
  if (!people.length) return <Hint>{empty}</Hint>;
  return (
    <ul role="list" className="m-0 flex list-none flex-col gap-s-1 p-0">
      {people.map((m) => (
        <li key={m.root_id} className="flex h-11 items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3">
          <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{m.display_name}</span>
          <span className="text-sm text-muted-foreground capitalize">{m.role}</span>
        </li>
      ))}
    </ul>
  );
}
