// The composer: a growing textarea, Enter to send and Shift+Enter for a new line, one send at a time (the text leaves the box
// at once and comes back if the send fails), the reply bar, staged files, the + menu and the typing signals. Ported from
// the old UI's views/chat.js (composer, send, togglePlusMenu, uploadFile) and the old UI's views/attachments.js (staged chips).
import { useEffect, useLayoutEffect, useRef, useState, type ChangeEvent, type KeyboardEvent } from 'react';
import { Link, Paperclip, Plus, SendHorizontal, Smile, UserPlus, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { openFileSendDialog } from '@/components/dialogs/file-send-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { contactLink } from '@/lib/actions';
import { wrapSelection } from '@/lib/markdown';
import { api } from '@/lib/api';
import { MAX_FILE_LABEL, guessMime, isOverLimit } from '@/lib/attachments';
import { useChat, type StagedFile } from '@/lib/chat';
import { useShell } from '@/lib/store';
import { MAX_ROWS, activeMention, candidatesFor, mentionArg, mentionText, picksInText, type Candidate, type MentionPick } from '@/lib/mentions';
import { pickFiles, sendFiles, sendMessage, sendReply } from '@/lib/chat-actions';
import { copyText } from '@/lib/clipboard';
import { has } from '@/lib/features';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import { toast } from '@/lib/toasts';
import { stopTyping, typed } from '@/lib/typing';
import { openEmojiPicker } from './emoji-picker';
import { IconBtn } from './parts';
import { MentionList } from './mention-list';
import { StagedChip } from './attachments';
import type { ChatView } from './use-chat-view';

// The markdown marker for a formatting shortcut: Ctrl+B bold, Ctrl+I italic, Ctrl+U underline, Ctrl+Shift+X strike.
function formatMarker(key: string, shift: boolean): string | null {
  const k = key.toLowerCase();
  if (shift) return k === 'x' ? '~~' : null;
  return ({ b: '**', i: '*', u: '__' } as Record<string, string>)[k] ?? null;
}

// Choose files from the native dialog. With file sharing on, they open the Share dialog; otherwise they are staged.
async function uploadFile(): Promise<void> {
  const picked = await pickFiles();
  if (picked.length === 0) return;
  const files: StagedFile[] = picked.map((f) => ({ path: f.path, name: f.name, size: f.size, mime: f.mime || guessMime(f.name) }));
  if (has('multi_source')) openFileSendDialog(files.map((f) => ({ path: f.path, name: f.name, size: f.size ?? 0 })));
  else useChat.getState().stageFiles(files);
}

async function copyContactLink(): Promise<void> {
  const link = await contactLink();
  if (link && (await copyText(link))) toast('Contact link copied', 'info');
}

export function Composer({ view }: { view: ChatView }) {
  const replyTo = useChat((s) => s.replyTo);
  const staged = useChat((s) => s.staged);
  const [text, setText] = useState('');
  // "@" autocomplete: the token under the caret, the highlighted row, and the members picked so far.
  const community = useShell((s) => s.community);
  const [mention, setMention] = useState<{ start: number; query: string } | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [picks, setPicks] = useState<MentionPick[]>([]);
  const me = useShell((s) => s.me);
  const matches: Candidate[] = mention && has('mentions') && view.target.kind === 'channel' && community
    ? candidatesFor(community.members, mention.query, community.can_mention_everyone === true, me?.root_id)
    : [];
  const rows = matches.slice(0, MAX_ROWS);
  const input = useRef<HTMLTextAreaElement>(null);
  const sending = useRef(false); // one send at a time
  // Link previews are off until the sender turns them on. The first text with a link asks once, here in the composer.
  const [previews, setPreviews] = useState<{ on: boolean; prompted: boolean } | null>(null);
  const [askPreviews, setAskPreviews] = useState(false);
  const overLimit = staged.some(isOverLimit);

  useEffect(() => {
    Promise.all([api.linkPreviews(), api.linkPreviewPrompted()])
      .then(([on, prompted]) => setPreviews({ on, prompted }))
      .catch(() => {});
  }, []);

  // A reply starts with the composer focused, as the reply's button does.
  useEffect(() => {
    if (replyTo) input.current?.focus();
  }, [replyTo]);

  // The box grows with its text up to the CSS maximum.
  useLayoutEffect(() => {
    const el = input.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  // Inserts an emoji at the caret, as typed text does, and puts the caret after it.
  const insertEmoji = (emoji: string) => {
    const el = input.current;
    if (!el) return;
    const start = el.selectionStart;
    const end = el.selectionEnd;
    const next = el.value.slice(0, start) + emoji + el.value.slice(end);
    setText(next);
    typed(view.target, next);
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(start + emoji.length, start + emoji.length);
    });
  };

  const send = async (answered = false) => {
    if (sending.current) return;
    const value = text;
    const trimmed = value.trim();
    const { staged: files, replyTo: reply } = useChat.getState();
    if (!trimmed && files.length === 0) return;
    if (!answered && files.length === 0 && previews && !previews.on && !previews.prompted && /https:\/\//.test(trimmed)) {
      setAskPreviews(true);
      return;
    }
    stopTyping(view.target);
    if (files.some(isOverLimit)) {
      toast(`Remove files over ${MAX_FILE_LABEL} before sending.`);
      return;
    }
    sending.current = true;
    setText('');
    let ok: boolean;
    try {
      const mentions = view.target.kind === 'channel' && has('mentions') ? mentionArg(picksInText(trimmed, picks)) : undefined;
      if (files.length) ok = await sendFiles(view.target, files.map((f) => f.path), trimmed || null, mentions);
      else if (reply) ok = await sendReply(view.target, reply.id, trimmed, mentions);
      else ok = await sendMessage(view.target, trimmed, mentions);
    } finally {
      sending.current = false;
    }
    const after = useChat.getState();
    if (ok) {
      setPicks([]);
      if (after.staged === files) after.clearStaged();
      if (after.replyTo === reply) after.cancelReply();
    } else {
      // Put the text back, unless something new was typed meanwhile.
      setText((current) => (current === '' ? value : current));
    }
    input.current?.focus();
  };

  // Turn link previews on, or not now. Either way the question is not asked again, and the message goes out.
  const answerPreviews = async (on: boolean) => {
    setAskPreviews(false);
    setPreviews({ on, prompted: true });
    try {
      if (on) await api.setLinkPreviews(true);
      await api.markLinkPreviewPrompted();
    } catch (err) {
      toast(err);
    }
    void send(true);
  };

  const onChange = (e: ChangeEvent<HTMLTextAreaElement>) => {
    setText(e.target.value);
    setMention(view.target.kind === 'channel' && has('mentions') ? activeMention(e.target.value, e.target.selectionStart) : null);
    setMentionIndex(0);
    typed(view.target, e.target.value);
  };

  // Puts the picked "@Name" in place of the typed token, and the caret after it.
  const pickMention = (row: Candidate) => {
    const el = input.current;
    if (!el || !mention) return;
    const end = mention.start + 1 + mention.query.length;
    const insert = `${mentionText(row.pick)} `;
    const next = el.value.slice(0, mention.start) + insert + el.value.slice(end);
    setText(next);
    setPicks((current) => [...current, row.pick]);
    setMention(null);
    typed(view.target, next);
    requestAnimationFrame(() => {
      el.focus();
      const caret = mention.start + insert.length;
      el.setSelectionRange(caret, caret);
    });
  };

  // Wraps the selected text in a markdown marker and keeps the selection on the same words.
  const wrapInput = (marker: string) => {
    const el = input.current;
    if (!el) return;
    const next = wrapSelection(text, el.selectionStart, el.selectionEnd, marker);
    setText(next.text);
    requestAnimationFrame(() => el.setSelectionRange(next.start, next.end));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && !e.altKey) {
      const marker = formatMarker(e.key, e.shiftKey);
      if (marker) {
        e.preventDefault();
        wrapInput(marker);
        return;
      }
    }
    if (rows.length) {
      // The autocomplete is open: the keys move through it, and Enter or Tab picks, before any of them sends.
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setMentionIndex((i) => (i + 1) % rows.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setMentionIndex((i) => (i - 1 + rows.length) % rows.length);
        return;
      }
      if ((e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) || e.key === 'Tab') {
        e.preventDefault();
        pickMention(rows[mentionIndex] ?? rows[0]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        setMention(null);
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send();
    } else if (e.key === 'Escape' && replyTo) {
      e.preventDefault();
      useChat.getState().cancelReply();
    }
  };

  // Server: invite. Direct conversation: copy my contact link, so the other person can add me without a shared server.
  const openPlusMenu = (anchor: HTMLElement) => {
    const r = anchor.getBoundingClientRect();
    const items: MenuEntry[] = [
      view.files ? { label: 'Upload a file', icon: Paperclip, onPick: () => void uploadFile() } : null,
      view.dm
        ? { label: 'Copy contact link', icon: Link, onPick: () => void copyContactLink() }
        : view.target.kind === 'channel'
          ? { label: 'Create invite', icon: UserPlus, onPick: () => openInviteDialog(view.target.kind === 'channel' ? view.target.communityId : '') }
          : null,
    ];
    openContextMenu(r.left, r.top - 8, items, { placeAbove: true });
  };

  return (
    <div className="composer-host">
      <div className="composer-stack">
        <div className="reply-bar" role="status" hidden={!replyTo}>
          {replyTo ? (
            <>
              <span className="reply-bar-text">
                Replying to <strong>{replyTo.authorName}</strong>
              </span>
              <IconBtn
                label="Cancel reply"
                title="Cancel reply (Esc)"
                icon={X}
                onClick={() => {
                  useChat.getState().cancelReply();
                  input.current?.focus();
                }}
              />
            </>
          ) : null}
        </div>
        <div className="staged" role="list" aria-label="Files to send" hidden={staged.length === 0}>
          {staged.map((f) => (
            <StagedChip
              key={f.path}
              file={f}
              onRemove={() => {
                useChat.getState().unstage(f.path);
                input.current?.focus();
              }}
            />
          ))}
        </div>
        {askPreviews ? (
          <div role="group" aria-label="Link previews" className="preview-ask">
            <span>Show link previews? Dissent fetches the link from your device, so the site sees your IP address.</span>
            <Button variant="ghost" onClick={() => void answerPreviews(false)}>Not now</Button>
            <Button onClick={() => void answerPreviews(true)}>Turn on</Button>
          </div>
        ) : null}
        <div className="composer">
          {rows.length ? <MentionList rows={rows} total={matches.length} query={mention?.query ?? ''} active={mentionIndex} onPick={pickMention} /> : null}
          <IconBtn lg label="More actions" title={`Attach a file (up to ${MAX_FILE_LABEL})`} icon={Plus} aria-haspopup="menu" onClick={(e) => openPlusMenu(e.currentTarget)} />
          <textarea ref={input} rows={1} placeholder={view.placeholder} aria-label="Message" value={text} onChange={onChange} onKeyDown={onKeyDown} />
          <IconBtn lg label="Insert emoji" title="Emoji" icon={Smile} onClick={(e) => openEmojiPicker(e.currentTarget, insertEmoji, 'Insert emoji')} />
          <IconBtn lg className="send" label="Send message" title="Send" icon={SendHorizontal} disabled={overLimit} onClick={() => void send()} />
        </div>
      </div>
    </div>
  );
}
