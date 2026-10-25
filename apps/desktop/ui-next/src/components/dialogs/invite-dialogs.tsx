// Invite links: a link to join a server, and the contact link that starts a direct conversation. Ported from
// openInviteDialog and openContactDialog in ui/views/dialogs.js.
import { AddressCard } from '@/components/address-card';
import { Button } from '@/components/ui/button';
import { contactLink, createFullInvite, createInvite } from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import { openLinkDialog } from './link-dialog';
import { QrCode } from './qr-code';

// Versions before short invites cannot open the short link. This copies the long one, which they can.
async function copyFullLink(communityId: string): Promise<void> {
  const link = await createFullInvite(communityId);
  if (link && (await copyText(link))) toast('Full invite link copied', 'info');
}

export function openInviteDialog(communityId: string): void {
  openLinkDialog({
    title: 'Invite people',
    description: 'Anyone with this link can join the server. Share it out of band.',
    load: async () => (await createInvite(communityId)) || undefined,
    extra: (
      <Button variant="outline" className="self-start" onClick={() => void copyFullLink(communityId)}>
        Copy full link (older versions)
      </Button>
    ),
  });
}

// The QR code encodes the contact link, so scanning it adds you. It appears once the link has loaded, and your address
// card sits under it.
export function openContactDialog(): void {
  openLinkDialog({
    title: 'My contact link',
    description: 'Share this so someone can message you directly, even without a shared server.',
    load: async () => (await contactLink()) || undefined,
    extra: (link) => {
      const me = useShell.getState().me;
      return (
        <>
          {link ? <QrCode text={link} label="QR code of your contact link" /> : null}
          {me?.root_id ? <AddressCard kind="user" id={me.root_id} label="Your address" /> : null}
        </>
      );
    },
  });
}
