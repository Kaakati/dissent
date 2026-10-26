// A message link in a body: dissent://message/... (see message-menu.ts copyLinkOf). Its "Jump to message" button opens the
// message when this device has it, and says so when it does not (see jump.ts).
import { Button } from '@/components/ui/button';
import type { MessageRef } from '@/lib/types';
import { openMessage } from './jump';

const MESSAGE_RE = /dissent:\/\/message\/[^\s<>"']+/g;
const NO_ACCESS = "You don't have access to this message";

// The message a link names. A channel link has the community and the channel; a direct or group link has `dm` or `group`
// and the conversation's id. Null when the link is not one of ours.
export function refOfLink(link: string): MessageRef | null {
  const parts = link.replace(/^dissent:\/\/message\//, '').split('/');
  if (parts.length !== 3 || parts.some((p) => !p)) return null;
  const [a, b, id] = parts;
  const base = { community: null, channel: null, peer: null, message_id: id, text: '', author_name: '', ms: 0 };
  if (a === 'dm' || a === 'group') return { ...base, peer: b };
  return { ...base, community: a, channel: b };
}

// The message links in a body, without the punctuation a sentence puts after one.
export function messageLinks(text: string | null | undefined): MessageRef[] {
  return [...String(text ?? '').matchAll(MESSAGE_RE)]
    .map((m) => refOfLink(m[0].replace(/[.,;:!?)\]}]+$/, '')))
    .filter((r): r is MessageRef => r !== null);
}

export function MessageLinks({ text }: { text: string }) {
  const refs = messageLinks(text);
  if (refs.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-s-2">
      {refs.map((ref, i) => (
        <Button key={`${i}-${ref.message_id}`} type="button" variant="secondary" size="sm" onClick={() => void openMessage(ref, NO_ACCESS)}>
          Jump to message
        </Button>
      ))}
    </div>
  );
}
