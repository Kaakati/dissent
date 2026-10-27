// The call chat's messages. A call's chat is the conversation the call sends into (send_call_message): a direct call's DM
// with the other person, a group call's group, and a voice channel's own channel. The messages are read through the chat
// API and refreshed on the engine's events for that conversation, so no backend field is needed. They are kept whole, as
// the chat API gives them, so the call chat renders them with the chat's own rows (call-body.tsx).
import { create } from 'zustand';
import { api } from './api';
import { MSG_LIMIT } from './chat';
import { useCallRoom } from './calls';
import { useShell, type ShellState } from './store';
import { toast } from './toasts';
import type { ChannelMessage, ChatTarget, ConversationMessage, EngineEvent, Id } from './types';

export type CallConversation =
  | { kind: 'channel'; communityId: Id; channelId: Id }
  | { kind: 'dm'; peer: Id }
  | { kind: 'group'; groupId: Id };

interface CallChatState {
  conversation: CallConversation | null; // the conversation the messages below belong to
  channel: ChannelMessage[]; // a voice channel's messages, oldest first
  direct: ConversationMessage[]; // a DM's or a group's messages, oldest first
}

export const useCallChat = create<CallChatState>()(() => ({ conversation: null, channel: [], direct: [] }));

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

// The chat target of a call's conversation, which the chat's rows and composer act on.
export function chatTargetOf(c: CallConversation | null): ChatTarget | null {
  const key = conversationKey(c);
  if (!c || !key) return null;
  return { ...c, key: `call:${key}` };
}

// Each load takes the next number. Only the newest load's reply is kept, so a slow older read cannot overwrite
// the list a newer one already stored.
let latestLoad = 0;

// Reads the conversation and puts its rows in the store. A slow read for a conversation the call has left is dropped.
export async function loadCallChat(): Promise<void> {
  const load = ++latestLoad;
  const s = useShell.getState();
  const conversation = callConversationOf(s, useCallRoom.getState().groupId);
  const key = conversationKey(conversation);
  if (!conversation) {
    useCallChat.setState({ conversation: null, channel: [], direct: [] });
    return;
  }
  try {
    let channel: ChannelMessage[] = [];
    let direct: ConversationMessage[] = [];
    if (conversation.kind === 'channel') {
      channel = (await api.channelMessages(conversation.communityId, conversation.channelId, MSG_LIMIT)) as ChannelMessage[];
    } else if (conversation.kind === 'dm') {
      direct = (await api.dmMessages(conversation.peer, MSG_LIMIT)) as ConversationMessage[];
    } else {
      direct = (await api.groupMessages(conversation.groupId, MSG_LIMIT)) as ConversationMessage[];
    }
    if (load !== latestLoad) return;
    if (conversationKey(callConversationOf(useShell.getState(), useCallRoom.getState().groupId)) !== key) return;
    useCallChat.setState({ conversation, channel, direct });
  } catch (err) {
    if (load === latestLoad) toast(err);
  }
}

// Re-reads the call chat when the engine reports a change in its conversation: a new message, or an edit, a delete
// or a reaction on one already there. Channels use messages_changed, DMs dm_changed, and groups conversation_changed.
export function refreshCallChat(ev: EngineEvent): void {
  const c = useCallChat.getState().conversation;
  if (!c) return;
  const matches =
    (c.kind === 'channel' && (ev.kind === 'message' || ev.kind === 'messages_changed') && ev.community_id === c.communityId && ev.channel_id === c.channelId) ||
    (c.kind === 'dm' && (ev.kind === 'dm' || ev.kind === 'dm_changed') && ev.peer === c.peer) ||
    (c.kind === 'group' && ((ev.kind === 'group' && ev.group_id === c.groupId) || (ev.kind === 'conversation_changed' && ev.id === c.groupId)));
  if (matches) void loadCallChat();
}

// Re-reads the call chat after an action in the chat (a send, an edit, a reaction), when a call is open.
export function reloadCallChat(): void {
  if (useCallChat.getState().conversation) void loadCallChat();
}
