// The call chat's messages. A call's chat is the conversation the call sends into (send_call_message): a direct call's DM
// with the other person, a group call's group, and a voice channel's own channel. The messages are read through the chat
// API and refreshed on the engine's events for that conversation, so no backend field is needed.
import { create } from 'zustand';
import { api } from './api';
import { MSG_LIMIT } from './chat';
import { useCallRoom } from './calls';
import { useShell, type ShellState } from './store';
import { toast } from './toasts';
import type { CallMessage, ChannelMessage, ConversationMessage, EngineEvent, Id } from './types';

export type CallConversation =
  | { kind: 'channel'; communityId: Id; channelId: Id }
  | { kind: 'dm'; peer: Id }
  | { kind: 'group'; groupId: Id };

interface CallChatState {
  conversation: CallConversation | null; // the conversation the rows below belong to
  messages: CallMessage[]; // oldest first
}

export const useCallChat = create<CallChatState>()(() => ({ conversation: null, messages: [] }));

export const conversationKey = (c: CallConversation | null): string | null =>
  c === null ? null : c.kind === 'channel' ? `ch:${c.communityId}:${c.channelId}` : `${c.kind}:${c.kind === 'dm' ? c.peer : c.groupId}`;

// The conversation of the call on this device. A group call is matched through the group this device joined, because
// its room carries no group id (useCallRoom); a DM call is the other person.
export function callConversationOf(s: ShellState, groupId: Id | null): CallConversation | null {
  const call = s.call;
  if (!call) return null;
  if (call.scope === 'channel') {
    if (!s.community || s.voiceChannelId === null) return null;
    return { kind: 'channel', communityId: s.community.id, channelId: s.voiceChannelId };
  }
  if (groupId && (call.scope === 'group' || call.peer === groupId)) return { kind: 'group', groupId };
  if (call.peer) return { kind: 'dm', peer: call.peer };
  return null;
}

// The rows of a conversation's messages, as the call chat shows them: text only, without deleted ones. A group's system
// lines (who was added) read as they do in the chat.
function fromChannel(items: ChannelMessage[]): CallMessage[] {
  return items.filter((m) => !m.deleted).map((m) => ({ id: m.id, root_id: m.author, author_name: m.author_name, ms: m.ms, text: m.text }));
}

function fromConversation(items: ConversationMessage[], names: (id: Id) => string): CallMessage[] {
  return items
    .filter((m) => !m.deleted)
    .map((m) => ({ id: m.id, root_id: m.from, author_name: names(m.from), ms: m.ms, text: m.text }));
}

// Reads the conversation and puts its rows in the store. A slow read for a conversation the call has left is dropped.
export async function loadCallChat(): Promise<void> {
  const s = useShell.getState();
  const conversation = callConversationOf(s, useCallRoom.getState().groupId);
  const key = conversationKey(conversation);
  if (!conversation) {
    useCallChat.setState({ conversation: null, messages: [] });
    return;
  }
  try {
    let messages: CallMessage[];
    if (conversation.kind === 'channel') {
      messages = fromChannel((await api.channelMessages(conversation.communityId, conversation.channelId, MSG_LIMIT)) as ChannelMessage[]);
    } else if (conversation.kind === 'dm') {
      const names = (id: Id) => (id === s.me?.root_id ? (s.me?.display_name ?? '') : s.peerName);
      messages = fromConversation((await api.dmMessages(conversation.peer, MSG_LIMIT)) as ConversationMessage[], names);
    } else {
      const members = s.group?.members ?? [];
      const names = (id: Id) => (id === s.me?.root_id ? (s.me?.display_name ?? '') : (members.find((m) => m.root_id === id)?.display_name ?? ''));
      messages = fromConversation((await api.groupMessages(conversation.groupId, MSG_LIMIT)) as ConversationMessage[], names);
    }
    if (conversationKey(callConversationOf(useShell.getState(), useCallRoom.getState().groupId)) !== key) return;
    useCallChat.setState({ conversation, messages });
  } catch (err) {
    toast(err);
  }
}

// Re-reads the call chat when the engine reports a message in its conversation.
export function refreshCallChat(ev: EngineEvent): void {
  const c = useCallChat.getState().conversation;
  if (!c) return;
  const matches =
    (ev.kind === 'message' && c.kind === 'channel' && ev.community_id === c.communityId && ev.channel_id === c.channelId) ||
    (ev.kind === 'dm' && c.kind === 'dm' && ev.peer === c.peer) ||
    (ev.kind === 'group' && c.kind === 'group' && ev.group_id === c.groupId);
  if (matches) void loadCallChat();
}
