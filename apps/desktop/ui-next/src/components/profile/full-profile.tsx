// The full profile, in a dialog: the address with its QR code, the mutual servers, the handles, and the safety number with
// Copy contact link and Verify safety number. Opened from the DM panel's "View full profile". Ported from openFullProfile
// in the old UI's views/dmpanel.js.
import { CircleCheck, Link, ShieldCheck, TriangleAlert } from 'lucide-react';
import { AddressCard } from '@/components/address-card';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { addressOf, shortOf } from '@/lib/address';
import { api } from '@/lib/api';
import { contactLink } from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { openDialog } from '@/lib/dialog';
import { toast } from '@/lib/toasts';
import type { UserProfileView } from '@/lib/types';
import { bannerBackground } from './banner';
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
  const copyLink = async () => {
    const link = await contactLink();
    if (link && (await copyText(link))) toast('Contact link copied', 'info');
  };
  return (
    <div className="flex flex-col gap-s-3">
      <div data-profile-banner className="aspect-[16/5] w-full rounded-lg" style={{ background: bannerBackground(peer, view) }} />
      <p className="font-mono text-sm leading-5 break-all text-muted-foreground" title={addressOf('user', peer)}>
        {`${view.display_name}·${shortOf('user', peer)}`}
      </p>
      <AddressCard kind="user" id={peer} label="Address" qr />
      <Heading>Mutual servers</Heading>
      {view.mutual_communities.length ? (
        <MutualList servers={view.mutual_communities} />
      ) : (
        <p className="text-sm leading-[19px] text-muted-foreground">No mutual servers.</p>
      )}
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
      <DialogFooter>
        <Button variant="ghost" onClick={() => void copyLink()}>
          <Link />
          Copy contact link
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
    title: view.display_name,
    content: (close) => <FullProfileBody peer={peer} view={view} close={close} />,
  });
}
