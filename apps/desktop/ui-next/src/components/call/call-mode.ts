// Which call board the main column shows for the open place: the call pane (a voice channel or a direct or group call
// this device is in), the lobby (a live call in the open conversation that this device has not joined), or nothing, so
// the chat shows. Ported from the call checks in the old UI's views/chat.js (liveCallFor, callHere) and right.js.
import { has } from '@/lib/features';
import { useCallRoom, useLobby } from '@/lib/calls';
import { useShell, type ShellState } from '@/lib/store';
import type { LiveCall } from '@/lib/types';

export type CallBoard = 'call' | 'lobby' | null;

// The live call of the open direct or group conversation, from the conversation list (or the open group).
export function liveCallOf(s: ShellState): LiveCall | null {
  if (!has('calls') || s.route.kind !== 'home' || s.page || !s.peer) return null;
  if (s.group) return s.group.live_call ?? null;
  return s.conversations.find((c) => c.kind === 'dm' && c.peer === s.peer)?.live_call ?? null;
}

// A channel call is open here when its voice channel is the one this device is connected to.
function channelCallHere(s: ShellState): boolean {
  return s.call?.scope === 'channel' && s.voiceChannelId !== null && s.voiceChannelId === s.voiceConnection?.channelId;
}

// A direct or group call is open here when it belongs to the open conversation. A group call is matched by the group
// this device joined, because the room does not carry the group's id.
function conversationCallHere(s: ShellState): boolean {
  const call = s.call;
  if (!call || !has('calls') || s.route.kind !== 'home' || s.page || !s.peer) return false;
  if (call.scope === 'dm') return call.peer === s.peer;
  if (call.scope === 'group') return useCallRoom.getState().groupId === s.peer;
  return false;
}

export function callBoardOf(s: ShellState, dismissed: string | null): CallBoard {
  if (!has('calls') || s.page) return null;
  if (s.route.kind === 'server') return s.voiceChannelId !== null && channelCallHere(s) ? 'call' : null;
  if (conversationCallHere(s)) return 'call';
  if (liveCallOf(s) && dismissed !== s.peer) return 'lobby';
  return null;
}

// The board for the open place, re-read when the place or the call changes.
export function useCallBoard(): CallBoard {
  const dismissed = useLobby((l) => l.dismissed);
  return useShell((s) => callBoardOf(s, dismissed));
}
