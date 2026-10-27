// The Saved Messages page, from Home: the messages this device has saved (Bookmark Message in a message's menu), newest
// first, each with Jump (opens its conversation and scrolls to it) and Remove. Local only: nothing here is sent.
// Each message is drawn with the chat's own row, read-only, so it reads as it does in the chat: formatting, mentions,
// link previews, invite cards and files. A bookmark keeps only the text, author and time, so the rest comes from the
// message itself when its conversation still holds it; otherwise the saved text is drawn alone.
import { useEffect, useState } from 'react';
import { Bookmark } from 'lucide-react';
import '@/components/chat/chat.css';
import { openMessage } from '@/components/chat/jump';
import { MessageRow } from '@/components/chat/message-row';
import { EmptyNode } from '@/components/chat/parts';
import { channelItems, conversationItems, type ChatView, type ViewItem } from '@/components/chat/use-chat-view';
import { Button } from '@/components/ui/button';
import { api } from '@/lib/api';
import { MSG_LIMIT } from '@/lib/chat';
import { listSaved, unsaveMessage } from '@/lib/saved';
import { useShell } from '@/lib/store';
import type { Bookmark as SavedMessage, ChannelMessage, ChatTarget, CommunityView, ConversationMessage, MessageRef } from '@/lib/types';

const GONE = 'This message is not available here: its conversation is closed or it is not in the loaded history.';

interface SavedRow {
  bookmark: SavedMessage;
  item: ViewItem;
  view: ChatView;
}

// Where a saved message is, as a chat target. A peer is a group when one of the conversations is that group.
function targetOf(ref: MessageRef): ChatTarget | null {
  if (ref.community && ref.channel) return { kind: 'channel', communityId: ref.community, channelId: ref.channel, key: `saved:ch:${ref.community}:${ref.channel}` };
  if (!ref.peer) return null;
  const group = useShell.getState().conversations.some((c) => c.kind === 'group' && c.id === ref.peer);
  return group ? { kind: 'group', groupId: ref.peer, key: `saved:group:${ref.peer}` } : { kind: 'dm', peer: ref.peer, key: `saved:dm:${ref.peer}` };
}

// The messages of a conversation, read once per page load. A conversation that cannot be read gives none.
async function readConversation(target: ChatTarget): Promise<ViewItem[]> {
  const me = useShell.getState().me;
  try {
    if (target.kind === 'channel') {
      return channelItems((await api.channelMessages(target.communityId, target.channelId, MSG_LIMIT)) as ChannelMessage[], me?.root_id ?? null);
    }
    const read = target.kind === 'dm' ? api.dmMessages(target.peer, MSG_LIMIT) : api.groupMessages(target.groupId, MSG_LIMIT);
    return conversationItems((await read) as ConversationMessage[], me, () => '');
  } catch {
    return [];
  }
}

// The row of a bookmark whose message is not in its conversation now: the saved text, author and time alone.
function savedItem(ref: MessageRef): ViewItem {
  return {
    id: ref.message_id, author: ref.author_name, authorName: ref.author_name, text: ref.text, ms: Number(ref.ms),
    edited: false, deleted: false, locked: false, beforeJoin: false, own: false, pending: false, queued: false,
    failed: false, failReason: null, retryable: false, system: false, replyTo: null, mentions: [], mentionsMe: false,
    mentionsEveryone: false, reactions: null, attachments: null, embeds: [],
  };
}

// The channels of a community, from the shell when it is the open one, else read from the engine. `null` when they
// cannot be read.
async function channelsOf(communityId: string): Promise<CommunityView['channels'] | null> {
  const open = useShell.getState().community;
  if (open?.id === communityId) return open.channels;
  try {
    return (await api.communityView(communityId)).channels;
  } catch {
    return null;
  }
}

// How a saved channel message is shown. Spoiler and age-restricted channels blur it until clicked, and so does a
// channel that cannot be read or found: a spoiler is never shown unblurred because its setting was missing. A channel
// with no setting at all (a backend from before channel visibility) is a plain one.
function visibilityOf(channels: CommunityView['channels'] | null, channelId: string): ChatView['visibility'] {
  const channel = channels?.find((c) => c.id === channelId);
  if (!channel) return 'spoiler';
  return (channel.visibility ?? 'default') === 'default' ? 'default' : 'spoiler';
}

// A read-only view to draw one saved message in.
function viewFor(target: ChatTarget, item: ViewItem, title: string, visibility: ChatView['visibility']): ChatView {
  return {
    target, dm: target.kind !== 'channel', group: target.kind === 'group', title, peer: null, topic: null,
    placeholder: '', items: [item], visibility, social: false, files: item.attachments !== null,
  };
}

async function rowsOf(list: SavedMessage[]): Promise<SavedRow[]> {
  const reads = new Map<string, Promise<ViewItem[]>>();
  const settings = new Map<string, Promise<CommunityView['channels'] | null>>();
  const rows: SavedRow[] = [];
  for (const bookmark of list) {
    const target = targetOf(bookmark.message);
    if (!target) continue;
    if (!reads.has(target.key)) reads.set(target.key, readConversation(target));
    const live = (await reads.get(target.key))?.find((m) => m.id === bookmark.message.message_id);
    // A direct conversation's rows name only this person here, so the others keep the name saved with the bookmark.
    const item = live && !live.deleted ? { ...live, authorName: live.authorName || bookmark.message.author_name } : savedItem(bookmark.message);
    let visibility: ChatView['visibility'] = 'default';
    let title = '';
    if (target.kind === 'channel') {
      if (!settings.has(target.communityId)) settings.set(target.communityId, channelsOf(target.communityId));
      const channels = (await settings.get(target.communityId)) ?? null;
      visibility = visibilityOf(channels, target.channelId);
      title = channels?.find((c) => c.id === target.channelId)?.name ?? '';
    }
    rows.push({ bookmark, item, view: viewFor(target, item, title, visibility) });
  }
  return rows;
}

export function SavedBody() {
  const [rows, setRows] = useState<SavedRow[] | null>(null);
  const [reads, setReads] = useState(0); // bumped after a Remove, so the list is read again

  useEffect(() => {
    let live = true;
    void listSaved()
      .then((list) => rowsOf([...list].sort((a, b) => b.saved_ms - a.saved_ms)))
      .then((next) => {
        if (live) setRows(next);
      });
    return () => {
      live = false;
    };
  }, [reads]);

  if (rows === null) return null;
  if (rows.length === 0) {
    return (
      <div className="flex min-h-0 items-center justify-center">
        <EmptyNode icon={Bookmark} title="No saved messages" text="Use Bookmark Message in a message's menu to keep it here." />
      </div>
    );
  }
  return (
    <ul aria-label="Saved messages" className="dissent-scroll flex min-h-0 flex-col gap-s-2 overflow-y-auto p-s-4">
      {rows.map(({ bookmark, item, view }) => (
        <li key={bookmark.key} className="flex items-start gap-s-3 rounded-lg bg-panel py-s-2 pr-s-3">
          <div className="min-w-0 flex-1">
            <MessageRow item={item} view={view} start communityId={view.target.kind === 'channel' ? view.target.communityId : null} readOnly />
          </div>
          <div className="flex shrink-0 gap-s-2 pt-s-2">
            <Button type="button" variant="secondary" size="sm" onClick={() => void openMessage(bookmark.message, GONE)}>
              Jump
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => void unsaveMessage(bookmark.key).then(() => setReads((n) => n + 1))}
            >
              Remove
            </Button>
          </div>
        </li>
      ))}
    </ul>
  );
}
