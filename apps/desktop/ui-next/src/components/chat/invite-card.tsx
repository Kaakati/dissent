// Invite links in a message: a card with the server's banner, icon, name and counts, and a Join button. The preview comes
// from the backend, which asks a member, so the client never fetches a link itself. Ported from the invite card in
// ui/views/chat.js.
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { invitePreview, joinCommunity } from '@/lib/actions';
import { hueOf, initials } from '@/lib/identity';
import { useShell } from '@/lib/store';
import type { InvitePreview } from '@/lib/types';
import { dayLabel } from '@/lib/time';
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
  const members = preview.member_count === 1 ? 'Member' : 'Members';
  const buttonLabel = preview.already_member ? 'Joined' : dead ? 'Invite expired or revoked' : joining ? 'Joining…' : 'Join';
  const onButton = preview.already_member
    ? () => void useShell.getState().openServer(preview.community)
    : () => void join();

  return (
    <article className="mt-1 grid w-full max-w-[400px] overflow-hidden rounded-[var(--r-1)] bg-sidebar shadow-[inset_0_0_0_1px_var(--border)]">
      <div className="flex h-16 items-center px-4" style={{ background: `oklch(45% 0.14 ${hueOf(preview.community)})` }}>
        {preview.icon && imageSrc(preview.icon) ? (
          <img className="size-12 rounded-full object-cover shadow-[0_0_0_3px_var(--sidebar)]" src={imageSrc(preview.icon) ?? undefined} alt="" />
        ) : (
          <span
            aria-hidden="true"
            className="grid size-12 place-items-center rounded-full font-bold text-text-strong shadow-[0_0_0_3px_var(--sidebar)]"
            style={{ background: `oklch(30% 0.1 ${hueOf(preview.community)})` }}
          >
            {initials(preview.name)}
          </span>
        )}
      </div>
      <div className="grid min-w-0 gap-1 px-4 pb-4 pt-3">
        <div className="break-words text-lg font-semibold text-text-strong">{preview.name}</div>
        <p className="m-0 flex flex-wrap items-center text-sm text-muted-foreground">
          <span className="mr-2 inline-block size-2 rounded-full bg-[var(--success)]" aria-hidden="true" />
          {`${preview.online_count} Online`}
          <span className="mx-2" aria-hidden="true">·</span>
          <span className="mr-2 inline-block size-2 rounded-full bg-[var(--offline)]" aria-hidden="true" />
          {`${preview.member_count} ${members}`}
        </p>
        <p className="m-0 text-xs text-muted-foreground">{`Created ${dayLabel(preview.created_ms)}`}</p>
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
      </div>
    </article>
  );
}
