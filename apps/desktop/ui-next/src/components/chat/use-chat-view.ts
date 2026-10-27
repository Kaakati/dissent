// The open conversation as one view model: the rows, the title, and the flags the header, the list and the composer read.
// Built from the shell store (who is open) and the chat store (its messages). Normalises a channel message and a
// direct or group message into one shape, as the current UI's views/chat.js `current()` does.
import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { shortOf } from '@/lib/address';
import { has } from '@/lib/features';
import { useChat } from '@/lib/chat';
import { useShell } from '@/lib/store';
import type {
  Attachment,
  ChannelMessage,
  ChatTarget,
  CommunityView,
  ConversationMessage,
  ConversationSummary,
  Embed,
  GroupSummary,
  Id,
  MentionRef,
  Reaction,
  ReplyRef,
} from '@/lib/types';

export interface ViewItem {
  id: Id;
  author: Id;
  authorName: string;
  text: string;
  ms: number;
  edited: boolean;
  deleted: boolean;
  locked: boolean;
  beforeJoin: boolean;
  own: boolean;
  pending: boolean;
  queued: boolean;
  failed: boolean; // refused, so it was not delivered; the sender may retry
  failReason: string | null;
  retryable: boolean; // a refused message can still be sent again
  system: boolean; // a membership line in a group
  replyTo: ReplyRef | null;
  mentions: MentionRef[];
  mentionsMe: boolean; // the message is highlighted for the viewer
  mentionsEveryone: boolean; // @everyone is a valid pill in the text
  reactions: Reaction[] | null;
  attachments: Attachment[] | null;
  embeds: Embed[];
}

export interface ChatView {
  target: ChatTarget;
  dm: boolean; // a direct conversation or a group, not a server channel
  group: boolean;
  title: string;
  peer: Id | null;
  topic: string | null;
  placeholder: string;
  items: ViewItem[];
  visibility: 'default' | 'spoiler' | 'age';
  // Replies and reactions exist only when the backend sends a `reactions` array; files, an `attachments` array.
  social: boolean;
  files: boolean;
}

export interface ShellSlice {
  me: { root_id: Id; display_name: string } | null;
  community: CommunityView | null;
  peerName: string;
  group: GroupSummary | null;
  conversations: ConversationSummary[];
}

// A direct conversation's name: the contact's name, else the name the shell was opened with, else a short address.
function peerNameOf(shell: ShellSlice, peer: Id): string {
  const known = shell.conversations.find((c): c is Extract<ConversationSummary, { kind: 'dm' }> => c.kind === 'dm' && c.peer === peer)?.display_name;
  return known || shell.peerName || `anon·${shortOf('user', peer)}`;
}

// A server channel's messages as rows. `meId` marks this person's own.
export function channelItems(messages: ChannelMessage[], meId: Id | null): ViewItem[] {
  return messages.map((m) => ({
    id: m.id,
    author: m.author,
    authorName: m.author_name,
    text: m.text,
    ms: Number(m.ms),
    edited: Boolean(m.edited),
    deleted: Boolean(m.deleted),
    locked: Boolean(m.locked),
    beforeJoin: Boolean(m.before_join),
    own: m.author === meId,
    pending: false,
    queued: Boolean(m.queued),
    failed: false,
    failReason: null,
    retryable: false,
    system: false,
    replyTo: m.reply_to ?? null,
    reactions: Array.isArray(m.reactions) ? m.reactions : null,
    attachments: Array.isArray(m.attachments) ? m.attachments : null,
    embeds: Array.isArray(m.embeds) ? m.embeds : [],
    mentions: Array.isArray(m.mentions) ? m.mentions : [],
    mentionsMe: m.mentions_me === true,
    mentionsEveryone: m.mentions_everyone === true,
  }));
}

// A direct conversation's or a group's messages as rows. `nameOf` names an author who is not this person.
export function conversationItems(
  messages: ConversationMessage[],
  me: { root_id: Id; display_name: string } | null,
  nameOf: (id: Id) => string,
): ViewItem[] {
  return messages.map((m) => {
    const own = m.from === me?.root_id;
    return {
      id: m.id,
      author: m.from,
      authorName: own ? (me?.display_name ?? '') : nameOf(m.from),
      text: m.text,
      ms: Number(m.ms),
      edited: Boolean(m.edited),
      deleted: Boolean(m.deleted),
      locked: false,
      beforeJoin: false,
      own,
      pending: Boolean(m.pending),
      queued: Boolean(m.queued),
      failed: m.delivery === 'failed',
      failReason: m.reason ?? null,
      retryable: m.retryable === true,
      system: m.system === true, // a membership line in a group, e.g. "added Omar and Rana."
      replyTo: m.reply_to ?? null,
      reactions: Array.isArray(m.reactions) ? m.reactions : null,
      attachments: Array.isArray(m.attachments) ? m.attachments : null,
      embeds: Array.isArray(m.embeds) ? m.embeds : [],
      mentions: Array.isArray(m.mentions) ? m.mentions : [],
      mentionsMe: m.mentions_me === true,
      mentionsEveryone: m.mentions_everyone === true,
    };
  });
}

// The view of `target`'s messages. The open chat and the call chat both build theirs here.
export function buildView(shell: ShellSlice, target: ChatTarget | null, messages: ChannelMessage[], dmMessages: ConversationMessage[]): ChatView | null {
  if (!target) return null;
  const me = shell.me;
  if (target.kind === 'channel') {
    const channel = shell.community?.channels.find((c) => c.id === target.channelId);
    if (!channel) return null;
    const items = channelItems(messages, me?.root_id ?? null);
    return {
      target,
      dm: false,
      group: false,
      title: channel.name,
      peer: null,
      topic: has('topics') ? (channel.topic ?? null) : null,
      placeholder: `Message #${channel.name}`,
      items,
      visibility: channel.visibility ?? 'default',
      social: items.some((m) => m.reactions !== null),
      files: items.some((m) => m.attachments !== null),
    };
  }

  const peer = target.kind === 'dm' ? target.peer : target.groupId;
  const name = target.kind === 'group' ? shell.peerName : peerNameOf(shell, peer);
  const members = shell.group?.members ?? [];
  const items = conversationItems(dmMessages, me, (id) => members.find((x) => x.root_id === id)?.display_name ?? name);
  return {
    target,
    dm: true,
    group: target.kind === 'group',
    title: name,
    peer,
    topic: null,
    placeholder: `Message ${name}`,
    items,
    visibility: 'default',
    social: items.some((m) => m.reactions !== null),
    files: items.some((m) => m.attachments !== null),
  };
}

// The open conversation's view, or null when the chat has none (Friends, a voice board, or a server still loading).
export function useChatView(): ChatView | null {
  const shell = useShell(
    useShallow((s) => ({
      me: s.me,
      community: s.community,
      peerName: s.peerName,
      group: s.group,
      conversations: s.conversations,
    })),
  );
  const target = useChat((s) => s.target);
  const messages = useChat((s) => s.messages);
  const dmMessages = useChat((s) => s.dmMessages);
  return useMemo(() => buildView(shell, target, messages, dmMessages), [shell, target, messages, dmMessages]);
}
