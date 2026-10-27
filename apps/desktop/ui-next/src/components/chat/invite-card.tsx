// Invite links in a message: a card with the server's banner, icon, name and counts, and a Join button. The preview comes
// from the backend, which asks a member, so the client never fetches a link itself. Ported from the invite card in
// the old UI's views/chat.js.
import { useEffect, useState } from 'react';
import { ServerCard } from '@/components/server-card';
import { Button } from '@/components/ui/button';
import { invitePreview, joinCommunity } from '@/lib/actions';
import { bannerOf } from '@/lib/banners';
import { useShell } from '@/lib/store';
import type { InvitePreview } from '@/lib/types';
import { cn } from '@/lib/utils';
import { imageSrc } from './attachments';

const INVITE_RE = /dissent:\/\/invite\/[^\s<>"']+/g;

// The invite links in a message, without the punctuation a sentence puts after one.
export function inviteLinks(text: string | null | undefined): string[] {
  return [...String(text ?? '').matchAll(INVITE_RE)].map((m) => m[0].replace(/[.,;:!?)\]}]+$/, ''));
}

// With one invite link and only a short label beside it ("Join X: <link>"), the body keeps the label and the card carries
// the link. Any other text is shown as written.
export function bodyText(text: string): string {
  if (inviteLinks(text).length !== 1) return text;
  const label = text.replace(INVITE_RE, '').replace(/^[\s:\u2013\u2014-]+|[\s:\u2013\u2014-]+$/g, '');
  return label.length <= 60 && !label.includes('\n') ? label : text;
}

// Previews by link for this session, so a remount shows the same card without asking the backend again. A failure is
// dropped, so the next mount asks again.
const previews = new Map<string, Promise<InvitePreview>>();

function previewOf(link: string): Promise<InvitePreview> {
  let pending = previews.get(link);
  if (!pending) {
    pending = invitePreview(link) as Promise<InvitePreview>;
    previews.set(link, pending);
    pending.catch(() => previews.delete(link));
  }
  return pending;
}

function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  return err instanceof Error ? err.message : String(err);
}

export function InviteCards({ text }: { text: string }) {
  return (
    <>
      {inviteLinks(text).map((link, i) => (
        <InviteCard key={`${i}-${link}`} link={link} />
      ))}
    </>
  );
}

type Loaded = { preview: InvitePreview } | { error: string } | null;

function InviteCard({ link }: { link: string }) {
  const [loaded, setLoaded] = useState<Loaded>(null);
  const [joining, setJoining] = useState(false);

  useEffect(() => {
    let live = true;
    previewOf(link).then(
      (preview) => live && setLoaded({ preview }),
      (err: unknown) => live && setLoaded({ error: errorText(err) }),
    );
    return () => {
      live = false;
    };
  }, [link]);

  if (!loaded) {
    return (
      <article className="mt-1 w-full max-w-[400px] rounded-[var(--r-1)] bg-sidebar p-4 shadow-[inset_0_0_0_1px_var(--border)]">
        <p className="m-0 text-sm text-muted-foreground">Loading invite…</p>
      </article>
    );
  }
  if ('error' in loaded) {
    return (
      <article className="mt-1 w-full max-w-[400px] rounded-[var(--r-1)] bg-sidebar p-4 shadow-[inset_0_0_0_1px_var(--border)]">
        <div className="font-semibold text-text-strong">Could not load this invite</div>
        <p className="m-0 mt-1 text-sm text-muted-foreground">{loaded.error}</p>
      </article>
    );
  }

  const preview = loaded.preview;
  const dead = preview.invite === 'revoked' || preview.invite === 'expired';
  const join = async () => {
    setJoining(true);
    const summary = await joinCommunity(link);
    if (!summary) setJoining(false);
    else setLoaded({ preview: { ...preview, already_member: true } });
  };
  const buttonLabel = preview.already_member ? 'Joined' : dead ? 'Invite expired or revoked' : joining ? 'Joining…' : 'Join';
  const onButton = preview.already_member
    ? () => void useShell.getState().openServer(preview.community)
    : () => void join();
  // The same card as Server Settings' preview, with the Join button under it.
  const banner = bannerOf(preview.banner ? imageSrc(preview.banner) : null, preview.banner_preset, preview.banner_poster ? imageSrc(preview.banner_poster) : null);

  return (
    <ServerCard
      className="mt-1 max-w-[360px]"
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
      footer={
        <Button
          className={cn(
            'mt-2 w-full justify-center font-semibold text-white',
            preview.already_member
              ? 'bg-[var(--joined-button)] text-text-strong hover:bg-[var(--joined-button)] hover:brightness-95'
              : 'bg-[var(--success-button)] hover:bg-[var(--success-button)] hover:brightness-95',
            'disabled:bg-[var(--offline)] disabled:text-text-strong',
          )}
          disabled={dead || joining}
          title={preview.already_member ? 'Go to server' : undefined}
          onClick={onButton}
        >
          {buttonLabel}
        </Button>
      }
    />
  );
}
