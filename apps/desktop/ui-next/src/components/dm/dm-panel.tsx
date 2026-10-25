// The DM profile panel in the right column: a banner with the contact status and More, the 80 px avatar with presence,
// the name and tag, the mutual servers (folded behind a count), "Contact since", and View full profile. Ported from
// ui/views/dmpanel.js. The panel is open while the profile panel toggle (lib/right.ts) is on.
import { Check, Clock, Ellipsis, type LucideIcon } from 'lucide-react';
import { openConversationMenu } from '@/components/menus/conversation-menu';
import { longDate, openFullProfile } from '@/components/profile/full-profile';
import { bannerBackground } from '@/components/profile/banner';
import { MutualToggle } from '@/components/profile/mutual-servers';
import { useUserProfile } from '@/components/profile/use-user-profile';
import { UserAvatar } from '@/components/user-avatar';
import { Button } from '@/components/ui/button';
import { addressOf, shortOf } from '@/lib/address';
import { useShell } from '@/lib/store';
import type { ContactsView, DirectSummary, Presence, UserProfileView } from '@/lib/types';

// The contact-status icon on the banner: a contact, or a request waiting either way. Nothing otherwise.
function contactStatus(contacts: ContactsView | null, peer: string): { label: string; icon: LucideIcon } | null {
  if (contacts?.contacts.some((c) => c.root_id === peer)) return { label: 'Contact', icon: Check };
  if (contacts?.outgoing.some((c) => c.root_id === peer) || contacts?.incoming.some((c) => c.root_id === peer)) {
    return { label: 'Request pending', icon: Clock };
  }
  return null;
}

// The presence of the person: their profile's, else the conversation row's.
function presenceOf(view: UserProfileView, row: DirectSummary | undefined): Presence | undefined {
  const from = (src: { status?: Presence; online?: boolean } | undefined): Presence | undefined => {
    if (src?.status) return src.status;
    if (typeof src?.online === 'boolean') return src.online ? 'online' : 'offline';
    return undefined;
  };
  return from(view) ?? from(row);
}

const BANNER_BUTTON = 'grid size-8 place-items-center rounded-full bg-black/50 text-text-strong hover:bg-black/70';

export function DmPanel({ peer }: { peer: string }) {
  const contacts = useShell((s) => s.contacts);
  const conv = useShell((s) => s.conversations.find((c): c is DirectSummary => c.kind === 'dm' && c.peer === peer));
  const load = useUserProfile(peer);
  const view = load.status === 'ready' ? load.view : null;
  const status = contactStatus(contacts, peer);
  const StatusIcon = status?.icon;

  if (!view) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="h-[120px] shrink-0" style={{ background: bannerBackground(peer) }} />
        {load.status === 'loading' ? <p className="p-s-4 text-sm text-muted-foreground">Loading profile…</p> : null}
      </div>
    );
  }

  const since = view.contact_since_ms ?? view.first_seen_ms;
  const presence = presenceOf(view, conv);

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto dissent-scroll">
      <div className="relative h-[120px] shrink-0" style={{ background: bannerBackground(peer) }}>
        <div className="absolute top-s-2 right-s-2 flex gap-s-2">
          {status && StatusIcon ? (
            <span role="img" aria-label={status.label} title={status.label} className="grid size-8 place-items-center rounded-full bg-black/50 text-text-strong">
              <StatusIcon className="size-4" aria-hidden="true" />
            </span>
          ) : null}
          <button
            type="button"
            aria-label="More"
            title="More"
            aria-haspopup="menu"
            className={BANNER_BUTTON}
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              openConversationMenu(r.left, r.bottom, conv ?? { kind: 'dm', peer, display_name: view.display_name, pinned: false });
            }}
          >
            <Ellipsis className="size-4" aria-hidden="true" />
          </button>
        </div>
      </div>
      <div className="flex min-h-0 flex-1 flex-col px-s-4 pb-s-4">
        <UserAvatar name={view.display_name} seed={peer} size="xl" presence={presence} className="-mt-10 ring-[6px] ring-panel" />
        <h3 className="mt-s-4 text-title leading-[30px] font-bold break-words text-text-strong">{view.display_name}</h3>
        <div className="mt-1 font-mono text-sm leading-5 break-all text-muted-foreground" title={addressOf('user', peer)}>
          {`${view.display_name}·${shortOf('user', peer)}`}
        </div>
        <div className="mt-s-2">
          <MutualToggle servers={view.mutual_communities} />
        </div>
        {since ? (
          <>
            <h4 className="mt-s-4 pt-s-4 text-meta font-semibold text-text-strong shadow-[inset_0_1px_0_var(--border)]">Contact since</h4>
            <p className="mt-1 text-ui text-text">{longDate(since)}</p>
          </>
        ) : null}
        <div className="mt-auto flex flex-col gap-s-2 pt-s-4">
          <Button variant="outline" className="w-full" onClick={() => void openFullProfile(peer)}>
            View full profile
          </Button>
        </div>
      </div>
    </div>
  );
}
