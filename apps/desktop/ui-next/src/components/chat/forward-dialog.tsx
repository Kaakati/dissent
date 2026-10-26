// The Forward dialog: pick a direct conversation, a group or a server channel, and the message is sent there as a quote.
// Attachments are not forwarded; the dialog says so. The sends are the chat's own (chat-actions.ts sendMessage).
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { sendMessage } from '@/lib/chat-actions';
import { openDialog } from '@/lib/dialog';
import { channelKey, convKey, useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { ChatTarget, CommunityView, ConversationSummary } from '@/lib/types';
import { Button } from '@/components/ui/button';

// The text a forward sends: "Forwarded from <author>", then the original's lines quoted.
export function forwardText(author: string, text: string): string {
  const head = `Forwarded from ${author || 'unknown'}`;
  if (!text) return head;
  return [head, ...text.split('\n').map((line) => `> ${line}`)].join('\n');
}

interface Destination {
  label: string;
  target: ChatTarget;
}

// A row per place, under a heading per kind. Channels load from each server, and a server that fails is left out.
function ForwardList({ text, author, close }: { text: string; author: string; close: () => void }) {
  const conversations = useShell((s) => s.conversations);
  const communities = useShell((s) => s.communities);
  const [channels, setChannels] = useState<Destination[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    void Promise.all(
      communities.map(async (c): Promise<Destination[]> => {
        try {
          const view = (await api.communityView(c.id)) as CommunityView;
          return view.channels
            .filter((ch) => !ch.voice)
            .map((ch) => ({
              label: `${view.name} · #${ch.name}`,
              target: { kind: 'channel', communityId: c.id, channelId: ch.id, key: channelKey(c.id, ch.id) } as const,
            }));
        } catch {
          return [];
        }
      }),
    ).then((parts) => {
      if (live) setChannels(parts.flat());
    });
    return () => {
      live = false;
    };
  }, [communities]);

  const dms: Destination[] = conversations
    .filter((c): c is Extract<ConversationSummary, { kind: 'dm' }> => c.kind === 'dm')
    .map((c): Destination => ({ label: c.display_name, target: { kind: 'dm', peer: c.peer, key: convKey(c) } }));
  const groups: Destination[] = conversations
    .filter((c): c is Extract<ConversationSummary, { kind: 'group' }> => c.kind === 'group')
    .map((c): Destination => ({ label: c.name, target: { kind: 'group', groupId: c.id, key: convKey(c) } }));

  const send = async (target: ChatTarget) => {
    setBusy(true);
    const ok = await sendMessage(target, forwardText(author, text));
    setBusy(false);
    if (ok) {
      toast('Message forwarded', 'info');
      close();
    }
  };

  const section = (heading: string, list: Destination[]) =>
    list.length === 0 ? null : (
      <div className="flex flex-col gap-s-1">
        <h3 className="px-s-2 pt-s-2 text-meta font-semibold tracking-wide text-faint uppercase">{heading}</h3>
        {list.map((d) => (
          <Button
            key={d.target.key}
            type="button"
            variant="ghost"
            disabled={busy}
            className="h-auto justify-start px-s-2 py-s-2 text-left font-medium text-text"
            onClick={() => void send(d.target)}
          >
            <span className="truncate">{d.label}</span>
          </Button>
        ))}
      </div>
    );

  return (
    <div className="flex max-h-[min(420px,60vh)] flex-col overflow-y-auto">
      {section('Direct messages', dms)}
      {section('Groups', groups)}
      {section('Channels', channels)}
      {dms.length + groups.length + channels.length === 0 ? <p className="p-s-2 text-sm text-muted-foreground">Nowhere to forward to yet.</p> : null}
    </div>
  );
}

export function openForwardDialog(author: string, text: string): void {
  openDialog({
    title: 'Forward Message',
    sub: 'Attachments are not forwarded.',
    content: (close) => <ForwardList text={text} author={author} close={close} />,
  });
}
