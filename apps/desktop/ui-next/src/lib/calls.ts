// Call actions: voice channels, live direct and group calls, ringing calls, the call chat, and the sounds that follow the
// call. Each action calls the engine and then writes the shell store, so the views only read. Ported from the calls
// section of the old UI's actions.js. Failures become toasts; nothing here throws into a view.
import { create } from 'zustand';
import { api } from './api';
import { has } from './features';
import { toast } from './toasts';
import { beginOutgoingRing, callChanged } from './sounds';
import { useShell } from './store';
import type { CallState, Id, IncomingCall } from './types';

// The group of a group call this device is in. A group call's room carries no peer, so the group is remembered here.
export const useCallRoom = create<{ groupId: Id | null }>()(() => ({ groupId: null }));

// The call that rings this device, if any. One ringing call at a time: a second one waits for the first to be answered.
export const useIncoming = create<{ call: IncomingCall | null }>()(() => ({ call: null }));

// `dissent://call-ringing`. The ringing call is shown until it is answered, declined or ends elsewhere.
export function ringIncoming(call: IncomingCall): void {
  if (!has('calls') || useIncoming.getState().call) return;
  useIncoming.setState({ call });
}

// A call that rang out, or was answered on another device, is no longer ringing here.
export function dismissRinging(callId: string): void {
  if (useIncoming.getState().call?.call_id === callId) useIncoming.setState({ call: null });
}

// The lobby: the conversation whose live-call lobby the user dismissed with Not now (cleared when the open conversation
// changes), and the mic choice the lobby's button and Join share.
export const useLobby = create<{ dismissed: Id | null; micOn: boolean }>()(() => ({ dismissed: null, micOn: true }));

// Sets the call on this device. The store keeps the voice connection only while the call is a channel call.
const applyCall = (next: CallState | null): void => useShell.getState().handleCallEvent(next);

// Sounds follow every change to the call, whichever path made it: joins, leaves and the ringing that ends.
export function watchCallSounds(): () => void {
  return useShell.subscribe((s, prev) => {
    if (s.call !== prev.call) callChanged(prev.call, s.call);
  });
}

// Clears the dismissed lobby when the open conversation changes, as the current UI's lobby does on another view.
export function watchLobbyDismissal(): () => void {
  return useShell.subscribe((s, prev) => {
    if (s.peer !== prev.peer || s.page !== prev.page || s.route.kind !== prev.route.kind) useLobby.setState({ dismissed: null });
  });
}

// Clicking a voice channel joins it. Another call on this device is left first, so this one replaces it.
export async function openVoiceChannel(channelId: Id): Promise<void> {
  const { community, voiceConnection, call } = useShell.getState();
  if (!has('calls') || !community) return;
  if (voiceConnection?.channelId === channelId && call) {
    useShell.setState({ voiceChannelId: channelId, page: null });
    return;
  }
  if (call) await useShell.getState().leaveCall();
  const joined = await joinChannel(community.id, channelId);
  useShell.setState({ voiceChannelId: joined ? channelId : null, page: null });
}

async function joinChannel(communityId: Id, channelId: Id): Promise<boolean> {
  try {
    applyCall((await api.joinCall(communityId, channelId)) as CallState);
    useShell.setState({ voiceConnection: { communityId, channelId } });
    useCallRoom.setState({ groupId: null });
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

// Joins a call that is already live in a direct conversation or a group (the lobby). `muted` mutes the mic on arrival.
export async function joinLiveCall(target: { groupId: Id } | { peer: Id }, muted: boolean): Promise<boolean> {
  try {
    const next = 'groupId' in target
      ? ((await api.joinGroupCall(target.groupId)) as CallState | null)
      : ((await api.joinDmCall(target.peer)) as CallState | null);
    useCallRoom.setState({ groupId: 'groupId' in target ? target.groupId : null });
    await applyWithMute(next, muted);
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

// Sets the call, and mutes it when the user chose to start muted.
async function applyWithMute(next: CallState | null, muted: boolean): Promise<void> {
  applyCall(next);
  if (muted && next && !next.muted) {
    await api.setMuted(true);
    applyCall({ ...next, muted: true });
  }
}

// The voice call button of a conversation. A direct call starts and rings the other person (start_call). A group call
// starts the group's call, which rings everyone, or joins the one already running (join_group_call).
export async function startConversationCall(target: { groupId: Id } | { peer: Id }): Promise<boolean> {
  try {
    if ('groupId' in target) {
      useCallRoom.setState({ groupId: target.groupId });
      applyCall((await api.joinGroupCall(target.groupId)) as CallState | null);
      return true;
    }
    const next = (await api.startCall(target.peer)) as CallState;
    if (next && next.participants.length === 0) beginOutgoingRing();
    applyCall(next);
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

// Answers a ringing call. Answering with the mic muted mutes it once the call connects.
export async function acceptIncoming(call: IncomingCall, muted: boolean): Promise<boolean> {
  try {
    await api.acceptCall(call.call_id);
    useCallRoom.setState({ groupId: call.scope === 'group' ? (call.group_id ?? null) : null });
    await applyWithMute((await api.callState()) as CallState | null, muted);
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

export async function declineIncoming(callId: string): Promise<boolean> {
  try {
    await api.declineCall(callId);
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

// Sends a quick reply to the caller, then declines the call.
export async function replyAndDecline(call: IncomingCall, text: string): Promise<boolean> {
  try {
    if (call.scope === 'group' && call.group_id) await api.sendGroupMessage(call.group_id, text);
    else if (call.peer) await api.sendDm(call.peer, text);
    await api.declineCall(call.call_id);
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}

// Sends a line to the call chat. The engine sends it into the call's conversation; the call state is then re-read.
export async function sendCallMessage(text: string): Promise<boolean> {
  try {
    await api.sendCallMessage(text);
    await useShell.getState().loadCallState();
    return true;
  } catch (err) {
    toast(err);
    return false;
  }
}
