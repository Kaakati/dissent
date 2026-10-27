// The full profile, in a dialog: the banner with the avatar cut into it, the name and tag, the badges, Member Since, the
// mutual servers and friends, the handles, the address with its QR code, and the safety number. The footer has Message,
// Call, Copy contact link and Verify safety number. Opened from the DM panel's "View Full Profile". Ported from
// openFullProfile in the old UI's views/dmpanel.js.
import { CircleCheck, Link, MessageSquare, Phone, Radio, ShieldCheck, TriangleAlert } from 'lucide-react';
import { AddressCard } from '@/components/address-card';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { UserAvatar } from '@/components/user-avatar';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { addressOf, shortOf } from '@/lib/address';
import { api } from '@/lib/api';
import { contactLink } from '@/lib/actions';
import { startConversationCall } from '@/lib/calls';
import { copyText } from '@/lib/clipboard';
import { has } from '@/lib/features';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { DirectSummary, Presence, UserProfileView } from '@/lib/types';
import { bannerBackground } from './banner';
import { MutualFriendsToggle } from './mutual-friends';
import { MutualList } from './mutual-servers';

// "Jul 4", the short date the safety line uses; "Jul 4, 2026", the long one "Contact since" uses.
export const shortDate = (ms: number | string): string => new Date(Number(ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
export const longDate = (ms: number | string): string => new Date(Number(ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });

// Amber when the key changed and nobody compared it again, green when a comparison is on record, muted before any.
export function SafetyLine({ view }: { view: UserProfileView }) {
  if (view.key_changed_ms) {
    return (
      <div className="mt-s-1 flex items-center gap-s-2 text-sm leading-[19px] text-text">
        <TriangleAlert className="size-4 shrink-0 text-warn" aria-hidden="true" />
        <span>{`Key changed ${shortDate(view.key_changed_ms)}. Compare again.`}</span>
      </div>
    );
  }
  if (view.verified_ms) {
    return (
      <div className="mt-s-1 flex items-center gap-s-2 text-sm leading-[19px] text-text">
        <CircleCheck className="size-4 shrink-0 text-success" aria-hidden="true" />
        <span>{`Verified ${shortDate(view.verified_ms)}`}</span>
      </div>
    );
  }
  return <p className="text-sm leading-[19px] text-muted-foreground">Not compared yet</p>;
}

// A section heading in the full profile.
function Heading({ children }: { children: string }) {
  return <h4 className="mt-s-2 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{children}</h4>;
}

function FullProfileBody({ peer, view, close }: { peer: string; view: UserProfileView; close: () => void }) {
  const handles = (view.handles ?? []).filter((h) => h.handle);
  const since = view.contact_since_ms ?? view.first_seen_ms;
  const hostsFor = view.hosts_for ?? [];
  // The same order as the DM panel's dot: the profile, then the direct conversation's row, then the server member's status.
  const conv = useShell((s) => s.conversations.find((c): c is DirectSummary => c.kind === 'dm' && c.peer === peer));
  const member = useShell((s) => s.community?.members.find((m) => m.root_id === peer));
  const presenceOf = (src: { status?: Presence; online?: boolean } | undefined): Presence | undefined =>
    src?.status ?? (typeof src?.online === 'boolean' ? (src.online ? 'online' : 'offline') : undefined);
  const presence = presenceOf(view) ?? presenceOf(conv) ?? presenceOf(member);
  const copyLink = async () => {
    const link = await contactLink();
    if (link && (await copyText(link))) toast('Contact link copied', 'info');
  };
  const message = () => {
    close();
    void useShell.getState().openDm(peer, view.display_name);
  };
  const call = () => {
    close();
    void startConversationCall({ peer });
  };
  return (
    <div className="-mx-s-5 -mt-s-5 flex flex-col">
      <div data-profile-banner className="h-[120px] w-full" style={{ background: bannerBackground(peer, view) }} />
      <div className="relative px-s-5 pb-s-2">
        <UserAvatar
          name={view.display_name}
          seed={peer}
          size="xl"
          presence={presence}
          presenceClass="right-[-3px] bottom-[-3px] size-[18px]"
          className="-mt-12 size-[96px] text-[30px] ring-[6px] ring-chat"
        />
        <h3 className="mt-s-2 text-head leading-6 font-bold break-words text-text-strong">{view.display_name}</h3>
        <p className="mt-0.5 font-mono text-sm leading-5 break-all text-muted-foreground" title={addressOf('user', peer)}>
          {`${view.display_name}·${shortOf('user', peer)}`}
        </p>
        {hostsFor.length || view.is_contact ? (
          <div className="mt-s-2 flex flex-wrap items-center gap-1.5 text-meta text-muted-foreground">
            {hostsFor.length ? (
              <span data-relay-host-badge className="inline-flex h-5 items-center gap-1 rounded-full bg-border-strong px-s-2 font-semibold text-link">
                <Radio className="size-3" aria-hidden="true" />
                Relay host
                <span className="font-normal text-muted-foreground">{`for ${hostsFor.map((c) => c.name).join(', ')}`}</span>
              </span>
            ) : null}
            {view.is_contact ? (
              <span className="inline-flex h-5 items-center rounded-full bg-border-strong px-s-2 font-semibold text-text">Contact</span>
            ) : null}
          </div>
        ) : null}
        {since ? (
          <>
            <Heading>Member Since</Heading>
            <p className="text-ui text-text">{longDate(since)}</p>
          </>
        ) : null}
        <Heading>Mutual servers</Heading>
        {view.mutual_communities.length ? (
          <MutualList servers={view.mutual_communities} />
        ) : (
          <p className="text-sm leading-[19px] text-muted-foreground">No mutual servers.</p>
        )}
        <Heading>Mutual friends</Heading>
        {(view.mutual_friends ?? []).length ? (
          <MutualFriendsToggle friends={view.mutual_friends ?? []} />
        ) : (
          <p className="text-sm leading-[19px] text-muted-foreground">No mutual friends.</p>
        )}
        <AddressCard kind="user" id={peer} label="Address" qr />
        <Button variant="ghost" size="sm" className="mt-s-1" onClick={() => void copyLink()}>
          <Link />
          Copy contact link
        </Button>
        <Heading>Handles</Heading>
      {handles.length ? (
        <ul className="flex flex-col gap-0.5">
          {handles.map((h) => (
            <li key={h.community_id} className="flex h-8 items-center justify-between gap-s-2 text-sm">
              <span className="font-medium text-text">{`@${h.handle}`}</span>
              <span className="truncate text-muted-foreground">{h.community_name}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-sm leading-[19px] text-muted-foreground">No handles claimed.</p>
      )}
        <Heading>Safety number</Heading>
        <SafetyLine view={view} />
      </div>
      <DialogFooter>
        {has('calls') ? (
          <Button variant="secondary" aria-label={`Start voice call with ${view.display_name}`} onClick={call}>
            <Phone />
            Call
          </Button>
        ) : null}
        <Button variant="secondary" onClick={message}>
          <MessageSquare />
          Message
        </Button>
        <Button
          onClick={() => {
            close();
            void openSafetyDialog(peer, { name: view.display_name, keyChangedMs: view.key_changed_ms });
          }}
        >
          <ShieldCheck />
          Verify safety number
        </Button>
      </DialogFooter>
    </div>
  );
}

// Loads the profile first; nothing opens when it cannot be read (the failure is a toast).
export async function openFullProfile(peer: string): Promise<void> {
  let view: UserProfileView;
  try {
    view = await api.userProfile(peer);
  } catch (err) {
    toast(err);
    return;
  }
  openDialog({
    title: 'Profile',
    content: (close) => <FullProfileBody peer={peer} view={view} close={close} />,
  });
}
