// One message: its gutter (avatar or time), author and time at the head of a group, the body with search marks and links,
// the reply reference, spoiler handling, attachments, link previews, reactions, the hover actions and the inline editor.
// Ported from ui/views/chat.js (messageRow, bodyNode, spoilerGuard, rowActions, editForm, replyRef, systemRow).
import { Fragment, useEffect, useRef, useState, type CSSProperties } from 'react';
import { Clock, Ellipsis, Pencil, Reply, SmilePlus, Trash2 } from 'lucide-react';
import { UserAvatar } from '@/components/user-avatar';
import { useChat } from '@/lib/chat';
import { editMessage } from '@/lib/chat-actions';
import { has } from '@/lib/features';
import { hueOf } from '@/lib/identity';
import { linkParts } from '@/lib/text';
import { clockLabel, stampLabel, whenLabel } from '@/lib/time';
import { useShell } from '@/lib/store';
import type { ChatTarget, ReplyRef } from '@/lib/types';
import { cn } from '@/lib/utils';
import { AttachmentView } from './attachments';
import { EmbedCards } from './embeds';
import { InviteCards, bodyText, inviteLinks } from './invite-card';
import { canDelete, openMessageMenu, removeMessage } from './message-menu';
import { Reactions, pickReaction } from './reactions';
import { IconBtn, highlight, jumpTo, spoilerGuard } from './parts';
import type { ChatView, ViewItem } from './use-chat-view';

// A membership line in a group: the actor's name in bold, then the engine's words, then the time.
export function SystemRow({ item }: { item: ViewItem }) {
  return (
    <article className="msg system" data-id={item.id}>
      <div className="gutter" />
      <div className="msg-content">
        <p className="body">
          <strong>{item.authorName}</strong>
          {` ${item.text} `}
          <time>{clockLabel(item.ms)}</time>
        </p>
      </div>
    </article>
  );
}

// The line above a reply: a connector from the avatar column, the original's author and a one-line snippet.
function ReplyRefLine({ reply }: { reply: ReplyRef }) {
  const missing = reply.text === null || reply.text === undefined;
  const missingText = reply.locked ? 'Original message is encrypted' : 'Original message was deleted';
  const label = missing ? missingText : `Replying to ${reply.author_name}: ${reply.text}`;
  return (
    <button type="button" className="reply-ref" title={label} aria-label={label} onClick={() => jumpTo(reply.id)}>
      {missing ? (
        <span className="reply-missing">{missingText}</span>
      ) : (
        <>
          <UserAvatar name={reply.author_name} seed={reply.author ?? reply.author_name} size="xs" />
          <span className="reply-author">{reply.author_name || 'unknown'}</span>
          <span className="reply-snippet">{reply.text}</span>
        </>
      )}
    </button>
  );
}

function AuthorButton({ item }: { item: ViewItem }) {
  return (
    <button type="button" className="author" style={{ '--hue': hueOf(item.author) } as CSSProperties}>
      {item.authorName || 'unknown'}
    </button>
  );
}

// The message body. Deleted text is a single line. In a spoiler channel the text blurs until someone clicks it.
function Body({ item, view, filter }: { item: ViewItem; view: ChatView; filter: string }) {
  const revealed = useChat((s) => s.revealed.has(item.id));
  if (item.deleted) return <p className="body deleted">message deleted</p>;
  const hidden = view.visibility === 'spoiler' && !revealed;
  const reveal = () => useChat.getState().reveal(item.id);
  return (
    <p
      className={cn('body', hidden && 'spoiler-hidden')}
      tabIndex={hidden ? 0 : undefined}
      role={hidden ? 'button' : undefined}
      aria-label={hidden ? 'Spoiler. Press to reveal' : undefined}
      onClick={hidden ? reveal : undefined}
      onKeyDown={
        hidden
          ? (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              reveal();
            }
          }
          : undefined
      }
    >
      {linkParts(bodyText(item.text)).map((part, i) =>
        part.href ? (
          <a key={i} href={part.href} target="_blank" rel="noopener noreferrer">
            {highlight(part.text, filter)}
          </a>
        ) : (
          <Fragment key={i}>{highlight(part.text, filter)}</Fragment>
        ),
      )}
      {item.edited ? <span className="edited"> (edited)</span> : null}
    </p>
  );
}

// The inline editor: Enter saves, Escape cancels. The draft lives in this row, so it survives a refresh.
function EditForm({ item, target }: { item: ViewItem; target: ChatTarget }) {
  const [text, setText] = useState(item.text);
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    input.current?.focus();
  }, []);
  const cancel = () => useChat.getState().beginEdit(null);
  const save = async () => {
    const next = text.trim();
    if (!next) return;
    if (await editMessage(target, item.id, next)) useChat.getState().beginEdit(null);
  };
  return (
    <div className="edit">
      <textarea
        ref={input}
        className="edit-input"
        rows={2}
        aria-label="Edit message"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            void save();
          } else if (e.key === 'Escape') {
            cancel();
          }
        }}
      />
      <div className="row">
        <button type="button" className="primary" onClick={() => void save()}>Save</button>
        <button type="button" className="ghost" onClick={cancel}>Cancel</button>
      </div>
    </div>
  );
}

// The hover actions: reply, reaction, edit, delete and the "…" menu.
function RowActions({ item, view }: { item: ViewItem; view: ChatView }) {
  if (item.deleted) return null;
  return (
    <div className="msg-actions">
      {view.social ? (
        <IconBtn label="Reply" title="Reply (R)" icon={Reply} onClick={() => useChat.getState().startReply(item.id, item.authorName || 'unknown')} />
      ) : null}
      {view.social ? (
        <IconBtn label="Add reaction" icon={SmilePlus} aria-haspopup="dialog" onClick={(e) => pickReaction(view.target, item.id, item.reactions, e.currentTarget)} />
      ) : null}
      {item.own ? <IconBtn label="Edit message" title="Edit" icon={Pencil} onClick={() => useChat.getState().beginEdit(item.id)} /> : null}
      {canDelete(item, view) ? (
        <IconBtn
          label="Delete message"
          title="Delete (Shift+click skips the confirm)"
          icon={Trash2}
          danger
          onClick={(e) => void removeMessage(view.target, item.id, e.shiftKey)}
        />
      ) : null}
      <IconBtn
        label="More message actions"
        title="More"
        icon={Ellipsis}
        aria-haspopup="menu"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          openMessageMenu(r.left, r.bottom + 4, item, view, e.currentTarget.closest<HTMLElement>('.msg'));
        }}
      />
    </div>
  );
}

export function MessageRow({
  item,
  view,
  start,
  communityId,
}: {
  item: ViewItem;
  view: ChatView;
  start: boolean;
  communityId: string | null;
}) {
  const ignored = useShell((s) => s.ignored.includes(item.author));
  const revealedIgnored = useChat((s) => s.revealedIgnored.has(item.id));
  const editing = useChat((s) => s.editingId === item.id);
  const filter = useChat((s) => s.filter);
  // Attachments and link previews of a spoiler message are blurred with its text, and one click reveals them all.
  const mediaHidden = useChat((s) => view.visibility === 'spoiler' && !s.revealed.has(item.id));
  const revealMedia = () => useChat.getState().reveal(item.id);
  if (ignored && !revealedIgnored) {
    return (
      <div className="ignored-row">
        <button type="button" onClick={() => useChat.getState().revealIgnored(item.id)}>Ignored message, click to show</button>
      </div>
    );
  }

  const body = editing ? <EditForm item={item} target={view.target} /> : <Body item={item} view={view} filter={filter} />;
  const pending = item.pending ? (
    <span className="pending-note">
      <Clock aria-hidden="true" />
      sending…
    </span>
  ) : item.queued ? (
    <span className="pending-note">
      <Clock aria-hidden="true" />
      {view.dm ? `${view.title} is offline. Dissent will deliver this when they're back.` : 'Waiting for a member to come online'}
    </span>
  ) : null;
  const reactions = view.social && item.reactions?.length && !item.deleted ? (
    <Reactions target={view.target} messageId={item.id} reactions={item.reactions} />
  ) : null;
  const files = view.files && item.attachments?.length && !item.deleted ? (
    <div className={cn('attachments', mediaHidden && 'spoiler-hidden')} {...spoilerGuard(mediaHidden, revealMedia)}>
      {item.attachments.map((a) => (
        <AttachmentView key={a.id} a={a} communityId={communityId} messageId={item.id} />
      ))}
    </div>
  ) : null;
  // A file-only message has no empty text line.
  const bodyOrNull = item.deleted || item.text || !files ? body : null;
  const cards = has('embeds') && !item.deleted && item.embeds.length ? (
    <EmbedCards embeds={item.embeds} hidden={mediaHidden} reveal={revealMedia} />
  ) : null;
  const invites = !item.deleted && inviteLinks(item.text).length ? <InviteCards text={item.text} /> : null;

  const lines = (
    <>
      {bodyOrNull}
      {cards}
      {invites}
      {files}
      {pending}
      {reactions}
    </>
  );

  return (
    <article
      className={cn('msg', start ? 'start' : 'cont', item.pending && 'pending', item.queued && 'queued')}
      data-id={item.id}
      tabIndex={-1}
      onContextMenu={
        editing
          ? undefined
          : (e) => {
            e.preventDefault();
            openMessageMenu(e.clientX, e.clientY, item, view, e.currentTarget);
          }
      }
    >
      {item.replyTo ? <ReplyRefLine reply={item.replyTo} /> : null}
      <div className="gutter">
        {start ? (
          <UserAvatar name={item.authorName} seed={item.author} size="lg" className="absolute top-0 left-s-4" />
        ) : (
          <span className="gutter-time">{clockLabel(item.ms)}</span>
        )}
      </div>
      <div className="msg-content">
        {start ? (
          <div className="msg-head">
            <AuthorButton item={item} />
            <time title={stampLabel(item.ms)}>{whenLabel(item.ms)}</time>
          </div>
        ) : null}
        {lines}
      </div>
      {editing ? null : <RowActions item={item} view={view} />}
    </article>
  );
}
