// The composer: a growing textarea, Enter to send and Shift+Enter for a new line, one send at a time (the text leaves the box
// at once and comes back if the send fails), the reply bar, staged files, the + menu and the typing signals. Ported from
// ui/views/chat.js (composer, send, togglePlusMenu, uploadFile) and ui/views/attachments.js (staged chips).
import { useEffect, useLayoutEffect, useRef, useState, type ChangeEvent, type KeyboardEvent } from 'react';
import { Link, Paperclip, Plus, SendHorizontal, Smile, UserPlus, X } from 'lucide-react';
import { openFileSendDialog } from '@/components/dialogs/file-send-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { contactLink } from '@/lib/actions';
import { MAX_FILE_LABEL, guessMime, isOverLimit } from '@/lib/attachments';
import { useChat, type StagedFile } from '@/lib/chat';
import { pickFiles, sendFiles, sendMessage, sendReply } from '@/lib/chat-actions';
import { copyText } from '@/lib/clipboard';
import { has } from '@/lib/features';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import { toast } from '@/lib/toasts';
import { stopTyping, typed } from '@/lib/typing';
import { openEmojiPicker } from './emoji-picker';
import { IconBtn } from './parts';
import { StagedChip } from './attachments';
import type { ChatView } from './use-chat-view';

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
  const input = useRef<HTMLTextAreaElement>(null);
  const sending = useRef(false); // one send at a time
  const overLimit = staged.some(isOverLimit);

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

  const send = async () => {
    if (sending.current) return;
    const value = text;
    const trimmed = value.trim();
    const { staged: files, replyTo: reply } = useChat.getState();
    if (!trimmed && files.length === 0) return;
    stopTyping(view.target);
    if (files.some(isOverLimit)) {
      toast(`Remove files over ${MAX_FILE_LABEL} before sending.`);
      return;
    }
    sending.current = true;
    setText('');
    let ok: boolean;
    try {
      if (files.length) ok = await sendFiles(view.target, files.map((f) => f.path), trimmed || null);
      else if (reply) ok = await sendReply(view.target, reply.id, trimmed);
      else ok = await sendMessage(view.target, trimmed);
    } finally {
      sending.current = false;
    }
    const after = useChat.getState();
    if (ok) {
      if (after.staged === files) after.clearStaged();
      if (after.replyTo === reply) after.cancelReply();
    } else {
      // Put the text back, unless something new was typed meanwhile.
      setText((current) => (current === '' ? value : current));
    }
    input.current?.focus();
  };

  const onChange = (e: ChangeEvent<HTMLTextAreaElement>) => {
    setText(e.target.value);
    typed(view.target, e.target.value);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
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
        <div className="composer">
          <IconBtn lg label="More actions" title={`Attach a file (up to ${MAX_FILE_LABEL})`} icon={Plus} aria-haspopup="menu" onClick={(e) => openPlusMenu(e.currentTarget)} />
          <textarea ref={input} rows={1} placeholder={view.placeholder} aria-label="Message" value={text} onChange={onChange} onKeyDown={onKeyDown} />
          <IconBtn lg label="Insert emoji" title="Emoji" icon={Smile} onClick={(e) => openEmojiPicker(e.currentTarget, insertEmoji, 'Insert emoji')} />
          <IconBtn lg className="send" label="Send message" title="Send" icon={SendHorizontal} disabled={overLimit} onClick={() => void send()} />
        </div>
      </div>
    </div>
  );
}
