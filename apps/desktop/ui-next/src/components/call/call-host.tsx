// Mounted once in App. It listens for ringing calls and the engine's call endings, follows the call for the join and leave
// sounds, and renders the incoming call. Ported from the call listeners in the old UI's main.js.
import { useEffect } from 'react';
import { onEngineEvent, listenTo } from '@/lib/api';
import { dismissRinging, ringIncoming, watchCallSounds, watchLobbyDismissal } from '@/lib/calls';
import { refreshCallChat } from '@/lib/call-chat';
import type { IncomingCall } from '@/lib/types';
import { IncomingCallDialog } from './incoming';

type Stop = () => void;

export function CallHost() {
  useEffect(() => {
    const stops: Stop[] = [watchCallSounds(), watchLobbyDismissal()];
    // A failed listener (no backend) resolves to a no-op, so the shell still renders.
    const unlisten: Promise<Stop>[] = [
      listenTo<IncomingCall>('dissent://call-ringing', ringIncoming).catch(() => () => {}),
      onEngineEvent((ev) => {
        if (ev.kind === 'call_missed' || ev.kind === 'call_answered_elsewhere') dismissRinging(ev.call_id);
        refreshCallChat(ev);
      }).catch(() => () => {}),
    ];
    return () => {
      stops.forEach((stop) => stop());
      unlisten.forEach((pending) => void pending.then((stop) => stop()));
    };
  }, []);

  return <IncomingCallDialog />;
}
