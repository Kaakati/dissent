// The chat pane's body: the message list, the typing row, the composer, and the drop overlay. The header is chat-header.tsx.
// The pane shows the open conversation (see App.tsx). Ported from the old UI's views/chat.js (initChat, initDrop, render).
import type { DragEvent } from 'react';
import './chat.css';
import { Paperclip } from 'lucide-react';
import { MAX_FILE_LABEL } from '@/lib/attachments';
import { useChat } from '@/lib/chat';
import { MessageList } from './message-list';
import { needsAgeGate } from './chat-notices';
import { Composer } from './composer';
import { TypingRow } from './typing-row';
import { useChatView } from './use-chat-view';

// Whether a drag carries files, so the window takes the drop instead of navigating to the file.
const hasFiles = (e: DragEvent): boolean => [...(e.dataTransfer?.types ?? [])].includes('Files');

export function ChatBody() {
  const view = useChatView();
  const ageSession = useChat((s) => s.ageSession);
  const gated = view ? needsAgeGate(view, ageSession) : false;
  const canAttach = Boolean(view?.files);
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      onDragOver={(e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = canAttach ? 'copy' : 'none';
      }}
      onDrop={(e) => {
        if (hasFiles(e)) e.preventDefault();
      }}
    >
      <MessageList view={view} gated={gated} />
      <TypingRow target={view?.target ?? null} />
      {view && !gated ? <Composer key={view.target.key} view={view} /> : null}
    </div>
  );
}

// The drop target shown while files are dragged over the window. It covers the whole pane (see MainArea's overlay).
export function ChatOverlay() {
  const dropping = useChat((s) => s.dropping);
  const view = useChatView();
  if (!dropping || !view) return null;
  return (
    <div className="drop-overlay" aria-hidden="true">
      <div className="drop-box">
        <Paperclip aria-hidden="true" />
        <p className="drop-title">{`Drop to send to ${view.dm ? view.title : `#${view.title}`}`}</p>
        <p className="muted">{`Up to ${MAX_FILE_LABEL}, end-to-end encrypted`}</p>
      </div>
    </div>
  );
}
