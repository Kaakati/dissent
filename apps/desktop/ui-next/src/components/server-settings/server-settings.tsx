// Server Settings: a full-page overlay built like Settings, opened from the server menu by the owner. The nav lists what
// Dissent has: Server Profile, then People (Members, Invites, Bans). Server Profile is a form beside a live preview card,
// the same card an invite shows. The name, banner, description and traits wait in an unsaved bar until Save Changes,
// which publishes only what changed; the icon applies at once, through the server icon flow. Leaving with unsaved
// changes shakes the bar instead of closing, as Discord does.
import { useCallback, useEffect, useRef, useState } from 'react';
import { Smile, X } from 'lucide-react';
import { create } from 'zustand';
import { BannerPicker } from '@/components/banner-picker';
import { InviteList } from '@/components/channel-settings/channel-settings';
import { openEmojiPicker } from '@/components/chat/emoji-picker';
import { ServerCard } from '@/components/server-card';
import { chooseImage } from '@/components/settings/avatar-editor';
import { BLOCK_TITLE, Block, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { api } from '@/lib/api';
import { clearCommunityIcon, saveServerProfile, setCommunityIcon, type ServerProfileChanges } from '@/lib/actions';
import { bannerOf, draftUrlOf, sameBanner, type BannerDraft } from '@/lib/banners';
import { useDialogs } from '@/lib/dialog';
import { imageOf } from '@/lib/images';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { CommunityView, Id, Trait } from '@/lib/types';
import { cn } from '@/lib/utils';

const MAX_NAME = 64; // the engine's limits, in characters
const MAX_DESCRIPTION = 300;
const MAX_LABEL = 24;
const SLOTS = 5;

export type Section = 'profile' | 'members' | 'invites' | 'bans';
const NAV: { key: Section; label: string; group: 'server' | 'people' }[] = [
  { key: 'profile', label: 'Server Profile', group: 'server' },
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
                  icon={imageOf(community.id)}
                  banner={draft.banner}
                  memberCount={members.length}
                  onlineCount={online}
                  createdMs={community.created_ms}
                  description={draft.description.trim()}
                  traits={traitsOf(draft.slots)}
                />
              </aside>
            </div>
          ) : null}
          {section === 'members' ? <PeopleList people={members} empty="No members yet." /> : null}
          {section === 'invites' ? <InviteList communityId={community.id} /> : null}
          {section === 'bans' ? <PeopleList people={community.members.filter((m) => m.banned)} empty="Nobody is banned." /> : null}
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
  const icon = imageOf(community.id);
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
          <Button onClick={() => chooseImage({ shape: 'rounded', onApply: (bytes) => setCommunityIcon(community.id, bytes) })}>Change Server Icon</Button>
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
