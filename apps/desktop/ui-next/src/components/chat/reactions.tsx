// The reactions under a message: a pill per emoji with its count, the mine state, and a button that opens the picker.
// Ported from the old UI's views/chat.js (reactionsNode, whoReacted).
import { SmilePlus } from 'lucide-react';
import { toggleReaction } from '@/lib/chat-actions';
import type { ChatTarget, Id, Reaction } from '@/lib/types';
import { cn } from '@/lib/utils';
import { openEmojiPicker } from './emoji-picker';

// "Grace, Ken and 2 others". `names` is optional in the contract; without it the count and `me` are used.
export function whoReacted(r: Reaction): string {
  const count = Number(r.count) || 0;
  const names = Array.isArray(r.names) ? r.names.filter(Boolean).slice(0, 3) : [];
  if (names.length === 0) {
    if (r.me) return count > 1 ? `You and ${count - 1} other${count === 2 ? '' : 's'}` : 'You';
    return `${count} reaction${count === 1 ? '' : 's'}`;
  }
  const rest = count - names.length;
  if (rest > 0) {
    const shown = names.length > 2 ? names.slice(0, 2) : names;
    const others = count - shown.length;
    return `${shown.join(', ')} and ${others} other${others === 1 ? '' : 's'}`;
  }
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

// Opens the picker from a message's add button or its menu. Choosing an emoji adds it, or removes it when it is ours.
export function pickReaction(target: ChatTarget, messageId: Id, reactions: Reaction[] | null, anchor: HTMLElement): void {
  openEmojiPicker(anchor, (emoji) => {
    const mine = reactions?.some((r) => r.emoji === emoji && r.me) ?? false;
    void toggleReaction(target, messageId, emoji, mine);
  });
}

export function Reactions({ target, messageId, reactions }: { target: ChatTarget; messageId: Id; reactions: Reaction[] }) {
  return (
    <div className="reactions" role="group" aria-label="Reactions">
      {reactions.map((r) => {
        const who = whoReacted(r);
        return (
          <button
            key={r.emoji}
            type="button"
            className={cn('reaction', r.me && 'mine')}
            aria-pressed={r.me ? 'true' : 'false'}
            aria-label={`${r.emoji} ${r.count}, ${who}`}
            title={who}
            onClick={() => void toggleReaction(target, messageId, r.emoji, Boolean(r.me))}
          >
            <span className="reaction-emoji">{r.emoji}</span>
            <span className="reaction-count">{r.count}</span>
          </button>
        );
      })}
      <button
        type="button"
        className="reaction add"
        aria-label="Add reaction"
        title="Add reaction"
        onClick={(e) => pickReaction(target, messageId, reactions, e.currentTarget)}
      >
        <SmilePlus aria-hidden="true" />
      </button>
    </div>
  );
}
