// The confirm dialogs for a dissent:// link. Each one shows what the link resolves to, from the backend, and acts only
// when its button is pressed. The server dialog's join and the add-friend flow do the work.
import { useEffect, useState } from 'react';
import { ServerCard } from '@/components/server-card';
import { imageSrc } from '@/components/chat/attachments';
import { UserAvatar } from '@/components/user-avatar';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { api } from '@/lib/api';
import { invitePreview, joinCommunity, sendContactRequest, userProfile } from '@/lib/actions';
import { bannerOf } from '@/lib/banners';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import type { Id, InvitePreview } from '@/lib/types';

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function InviteBody({ link, close }: { link: string; close: () => void }) {
  const [preview, setPreview] = useState<InvitePreview | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let live = true;
    invitePreview(link).then(
      (p) => live && setPreview(p),
      (err: unknown) => live && setError(errorText(err)),
    );
    return () => {
      live = false;
    };
  }, [link]);

  if (error) {
    return (
      <>
        <p role="alert" className="text-sm text-danger">{error}</p>
        <DialogFooter>
          <Button variant="secondary" onClick={close}>Cancel</Button>
        </DialogFooter>
      </>
    );
  }
  if (!preview) return <p role="status" className="text-sm text-muted-foreground">Looking up the server…</p>;

  const banner = bannerOf(
    preview.banner ? imageSrc(preview.banner) : null,
    preview.banner_preset,
    preview.banner_poster ? imageSrc(preview.banner_poster) : null,
  );
  const open = () => {
    void useShell.getState().openServer(preview.community);
    close();
  };
  const join = async () => {
    if (await joinCommunity(link)) close();
  };

  return (
    <>
      <p className="text-sm text-muted-foreground">
        A link to join <strong className="text-text-strong">{preview.name}</strong>. Nothing happens until you press a button here.
      </p>
      <ServerCard
        className="max-w-[360px]"
        variant="invite"
        id={preview.community}
        name={preview.name}
        icon={preview.icon ? imageSrc(preview.icon) : null}
        iconPoster={preview.icon_poster ? imageSrc(preview.icon_poster) : null}
        banner={banner}
        memberCount={preview.member_count}
        onlineCount={preview.online_count}
        createdMs={preview.created_ms}
        description={preview.description}
        traits={preview.traits}
        boost={{ level: preview.boost_level ?? 0, hosts: preview.boost_hosts ?? 0 }}
      />
      {preview.already_member ? (
        <p className="text-sm text-muted-foreground">You're already a member of {preview.name}.</p>
      ) : preview.invite !== 'live' ? (
        <p role="alert" className="text-sm text-danger">This invite is {preview.invite}. Ask for a new one.</p>
      ) : null}
      <DialogFooter>
        <Button variant="secondary" onClick={close}>Cancel</Button>
        {preview.already_member ? (
          <Button onClick={open}>Open server</Button>
        ) : (
          <Button onClick={() => void join()} disabled={preview.invite !== 'live'}>Join server</Button>
        )}
      </DialogFooter>
    </>
  );
}

function ContactBody({ link, id, close }: { link: string; id: Id; close: () => void }) {
  const [name, setName] = useState<string | null>(null);
  const [safety, setSafety] = useState('');
  const isContact = useShell((s) => Boolean(s.contacts?.contacts.some((c) => c.root_id === id)));

  useEffect(() => {
    let live = true;
    void userProfile(id).then((view) => {
      if (live && view) setName(view.display_name);
    });
    api.safetyNumber(id).then(
      (s) => live && setSafety(String(s)),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [id]);

  const message = () => {
    void useShell.getState().openDm(id, name ?? '');
    close();
  };
  const request = async () => {
    if (await sendContactRequest(link)) close();
  };

  return (
    <>
      <div className="flex items-center gap-s-3">
        <UserAvatar name={name ?? 'Unknown person'} seed={id} size="lg" />
        <div className="min-w-0">
          <p className="truncate text-base font-semibold text-text-strong">{name ?? 'Looking them up…'}</p>
          <p className="text-sm text-muted-foreground">{isContact ? 'Already a contact' : 'Not a contact yet'}</p>
        </div>
      </div>
      <p className="text-xs text-muted-foreground">Safety number</p>
      <p className="break-all font-mono text-sm text-text">{safety || '…'}</p>
      <DialogFooter>
        <Button variant="secondary" onClick={close}>Cancel</Button>
        {isContact ? (
          <Button onClick={message}>Message</Button>
        ) : (
          <Button onClick={() => void request()}>Send contact request</Button>
        )}
      </DialogFooter>
    </>
  );
}

export function openInviteDialog(link: string): void {
  openDialog({ title: 'Join server', content: (close) => <InviteBody link={link} close={close} /> });
}

export function openContactDialog(link: string, id: Id): void {
  openDialog({ title: 'Add contact', content: (close) => <ContactBody link={link} id={id} close={close} /> });
}
