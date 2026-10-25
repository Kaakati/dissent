// The typing signals the composer sends. The engine throttles and expires them too; this throttle only keeps keystrokes
// cheap. The names on screen come from the shell store (`typing`, filled by dissent://typing-changed). Ported from
// ui/typing.js.
import { api } from './api';
import type { ChatTarget, TypingTarget } from './types';

const SEND_EVERY_MS = 3000;

const lastSent = new Map<string, number>();

// The engine's form of a chat target.
export function typingTarget(target: ChatTarget): TypingTarget {
  switch (target.kind) {
    case 'channel': return { kind: 'channel', communityId: target.communityId, channelId: target.channelId };
    case 'dm': return { kind: 'dm', peer: target.peer };
    case 'group': return { kind: 'group', groupId: target.groupId };
  }
}

// Called on each change to the composer. Empty text means the composer was cleared, so the typing stops.
export function typed(target: ChatTarget, text: string): void {
  if (text.trim() === '') {
    stopTyping(target);
    return;
  }
  const now = Date.now();
  if (now - (lastSent.get(target.key) ?? -Infinity) < SEND_EVERY_MS) return;
  lastSent.set(target.key, now);
  api.typing(typingTarget(target)).catch(() => {});
}

// Called when a message is sent, or the composer is cleared.
export function stopTyping(target: ChatTarget): void {
  lastSent.delete(target.key);
  api.stopTyping(typingTarget(target)).catch(() => {});
}
