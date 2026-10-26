// The lobby (the old UI's views/lobby.js): shown in place of the chat when a direct or group conversation has a live call this device
// is not in. The header carries the "Call in progress" pill and Join. The body has the self preview with the mic meter,
// the microphone and speaker choices, the route note and Join. The right column lists who is in the call and who is not.
// The camera is off until the video batch lands, and the preview says so.
import { useEffect, useRef, useState } from 'react';
import { Mic, MicOff, ShieldCheck } from 'lucide-react';
import { api } from '@/lib/api';
import { joinLiveCall, useLobby } from '@/lib/calls';
import { chooseDevice } from '@/lib/voice';
import { clockLabel } from '@/lib/time';
import { useShell } from '@/lib/store';
import { Button } from '@/components/ui/button';
import { UpdateButton } from '@/components/settings/update-dialog';
import { HeaderSearch } from '@/components/chat/chat-header';
import { UserAvatar } from '@/components/user-avatar';
import { liveCallOf } from './call-mode';
import type { AudioDevices, LiveCall, LiveParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';

const METER_BARS = 16;

// The current time, refreshed each minute, so the call's age reads right without calling Date.now() during render.
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

// The name of the open conversation: the group, or the person of a direct call.
const useConversationName = (): string => useShell((s) => s.group?.name ?? s.peerName);

// Joins the live call of the open conversation, with the mic as the lobby's button left it.
function joinLobby(): Promise<boolean> {
  const s = useShell.getState();
  const muted = !useLobby.getState().micOn;
  return joinLiveCall(s.group ? { groupId: s.group.id } : { peer: s.peer ?? '' }, muted);
}

// The header's pill and Join button, in place of the chat's header row.
export function LobbyHeader() {
  const name = useConversationName();
  const live = useShell(liveCallOf);
  const group = useShell((s) => s.group);
  const peer = useShell((s) => s.peer);
  const members = group?.members?.length ?? 0;
  return (
    <div className="chat-header">
      <div className="chat-title">
        {group ? (
          <GroupAvatars members={group.members} />
        ) : (
          <UserAvatar name={name} seed={peer} size="sm" />
        )}
        <h2 title={name}>{name}</h2>
        {group ? <span className="title-meta">{`${members} member${members === 1 ? '' : 's'}`}</span> : null}
        {group ? <span className="pill ok">Members only</span> : null}
        <span role="status" className="pill ok">
          <span className="size-2 rounded-full bg-success" aria-hidden="true" />
          {live?.started_ms ? `Call in progress · ${clockLabel(live.started_ms)}` : 'Call in progress'}
        </span>
      </div>
      <div className="header-tools">
        <Button onClick={() => void joinLobby()} className="h-8 bg-success px-s-4 text-white hover:bg-success/90">
          Join
        </Button>
      </div>
      <div className="update-host">
        <UpdateButton />
      </div>
      <HeaderSearch />
    </div>
  );
}

// Two members of a group, overlapping, as the group title shows them.
function GroupAvatars({ members }: { members: { root_id: string; display_name: string }[] }) {
  const [a, b] = members;
  return (
    <span className="relative size-8 shrink-0">
      {a ? <UserAvatar name={a.display_name} seed={a.root_id} size="xs" className="absolute top-0 left-0 size-5 text-[7px]" /> : null}
      {b ? <UserAvatar name={b.display_name} seed={b.root_id} size="xs" className="absolute right-0 bottom-0 size-5 text-[7px] shadow-[0_0_0_2px_var(--chat)]" /> : null}
    </span>
  );
}

// The lobby body: the question, the preview, the devices, the route note and the join row.
export function LobbyBody() {
  const live = useShell(liveCallOf);
  const now = useNow();
  const name = useConversationName();
  const group = useShell((s) => s.group);
  const micOn = useLobby((l) => l.micOn);
  const dismiss = () => useLobby.setState({ dismissed: useShell.getState().peer });
  if (!live) return null;
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
      <section aria-label="Get ready to join" className="mx-auto flex w-full max-w-[760px] flex-col gap-s-4 px-s-6 py-s-8">
        <h2 className="text-[25px] leading-8 font-bold text-text-strong">Join the call?</h2>
        <p className="text-ui leading-[21px] text-muted-foreground">{lobbySubtitle(live, name, group?.members.length ?? 0, group !== null, now)}</p>
        <div className="grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)] gap-s-4 max-[900px]:grid-cols-1">
          <Preview />
          <Devices />
        </div>
        <div className="flex items-start gap-s-2 rounded-lg bg-panel px-s-4 py-3 text-ui text-muted-foreground">
          <ShieldCheck className="mt-0.5 size-4 shrink-0 text-success-text" aria-hidden="true" />
          <span>{routeNote(live)}</span>
        </div>
        <div className="flex items-center justify-between gap-s-4">
          <Button variant="ghost" onClick={dismiss} className="h-10">
            Not now
          </Button>
          <div className="flex items-center gap-s-3">
            <span className="text-meta text-muted-foreground">{`${micOn ? 'Mic on' : 'Mic off'} · camera off`}</span>
            <Button onClick={() => void joinLobby()} className="h-10 min-w-[120px] bg-success text-white hover:bg-success/90">
              Join call
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}

function lobbySubtitle(live: LiveCall, name: string, members: number, group: boolean, now: number): string {
  const minutes = Math.max(1, Math.round((now - (live.started_ms ?? now)) / 60000));
  const started = live.started_by ?? (group ? 'Someone' : name);
  const inCall = live.participants?.length ?? 0;
  return group
    ? `${started} started a voice call ${minutes} minutes ago. ${inCall} of the group’s ${members} members are in.`
    : `${started} started a voice call ${minutes} minutes ago.`;
}

// The route note: end-to-end encrypted, and through whom when a person forwards the call.
function routeNote(live: LiveCall): string {
  const forwarder = live.participants?.find((p) => p.forwarder)?.display_name;
  return forwarder
    ? `End-to-end encrypted. You’ll connect through ${forwarder}, who forwards everyone’s sealed audio and can’t listen in.`
    : 'End-to-end encrypted. You’ll connect directly to everyone in the call.';
}

// The self preview: the avatar with the camera note, the mic meter and the mic button.
function Preview() {
  const me = useShell((s) => s.me);
  const micOn = useLobby((l) => l.micOn);
  const lit = useMicMeter(micOn);
  const toggle = () => useLobby.setState({ micOn: !useLobby.getState().micOn });
  return (
    <div className="relative flex min-h-[300px] flex-col items-center justify-center gap-s-3 rounded-[10px] bg-[#0f1012]">
      <span className="absolute top-s-3 left-s-3 rounded bg-black/60 px-2 py-1 text-[11px] leading-4 font-semibold text-white">
        Only you can see this preview
      </span>
      <div className="flex flex-col items-center gap-s-2">
        <UserAvatar name={me?.display_name ?? '?'} seed={me?.root_id} size="xl" />
        <span className="text-meta text-muted-foreground">Your camera is off</span>
      </div>
      <div className="absolute right-s-3 bottom-s-3 left-s-3 flex items-center justify-between gap-s-3">
        <div role="meter" aria-label="Microphone level" aria-valuemin={0} aria-valuemax={METER_BARS} aria-valuenow={lit} className="flex h-3.5 items-center gap-0.5">
          {Array.from({ length: METER_BARS }, (_, i) => (
            <span
              key={i}
              className={cn(
                'h-3 w-1.5 origin-center rounded-[1px] transition-transform duration-75 motion-reduce:transition-none',
                i < lit ? (i >= 12 ? 'scale-y-100 bg-[#e0a030]' : 'scale-y-100 bg-success') : 'scale-y-50 bg-elevated',
              )}
            />
          ))}
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={toggle}
          aria-label={micOn ? 'Mute microphone' : 'Unmute microphone'}
          aria-pressed={!micOn}
          title={micOn ? 'Mute' : 'Unmute'}
        >
          {micOn ? <Mic className="size-4" /> : <MicOff className="size-4" />}
        </Button>
      </div>
    </div>
  );
}

// The number of meter bars lit, from the microphone of the lobby. Without microphone access the meter stays empty.
function useMicMeter(micOn: boolean): number {
  const [lit, setLit] = useState(0);
  const micRef = useRef(micOn);
  useEffect(() => {
    micRef.current = micOn;
  }, [micOn]);
  useEffect(() => {
    let alive = true;
    let frame = 0;
    let stream: MediaStream | null = null;
    let ctx: AudioContext | null = null;
    const release = () => {
      cancelAnimationFrame(frame);
      stream?.getTracks().forEach((t) => t.stop());
      void ctx?.close().catch(() => {});
    };
    void (async () => {
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        if (!alive) return release();
        ctx = new AudioContext();
        const analyser = ctx.createAnalyser();
        analyser.fftSize = 256;
        ctx.createMediaStreamSource(stream).connect(analyser);
        const data = new Uint8Array(analyser.fftSize);
        const tick = () => {
          if (!alive) return;
          analyser.getByteTimeDomainData(data);
          let peak = 0;
          for (const v of data) peak = Math.max(peak, Math.abs(v - 128));
          const level = micRef.current ? Math.min(1, peak / 64) : 0;
          setLit(Math.round(level * METER_BARS));
          frame = requestAnimationFrame(tick);
        };
        tick();
      } catch {
        // No microphone access: the meter stays empty.
      }
    })();
    return () => {
      alive = false;
      release();
    };
  }, []);
  return lit;
}

// The microphone and speaker choices, saved as they change.
function Devices() {
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const load = () => {
    api.audioDevices().then(setDevices).catch(() => setDevices(null));
  };
  useEffect(load, []);
  if (!devices) return <div className="flex flex-col gap-s-3" />;
  return (
    <div className="flex flex-col justify-center gap-s-4">
      <DeviceField label="Microphone" id="dev-microphone" list={devices.inputs} chosen={devices.input} onChoose={(name) => void chooseDevice('input', name).then(load)} />
      <DeviceField label="Speaker" id="dev-speaker" list={devices.outputs} chosen={devices.output} onChoose={(name) => void chooseDevice('output', name).then(load)} />
    </div>
  );
}

function DeviceField({
  label,
  id,
  list,
  chosen,
  onChoose,
}: {
  label: string;
  id: string;
  list: AudioDevices['inputs'];
  chosen: string | null;
  onChoose: (name: string | null) => void;
}) {
  return (
    <div className="flex flex-col gap-s-1">
      <label htmlFor={id} className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{label}</label>
      <select
        id={id}
        value={chosen ?? ''}
        onChange={(e) => onChoose(e.target.value || null)}
        className="h-9 rounded-lg border border-border bg-rail px-s-3 text-ui text-text focus-visible:outline-2 focus-visible:outline-ring"
      >
        <option value="">System default</option>
        {list.map((d) => (
          <option key={d.name} value={d.name}>{d.name}</option>
        ))}
      </select>
    </div>
  );
}

// The right column while the lobby is open: who is in the call and who is not.
export function LobbyRoster() {
  const live = useShell(liveCallOf);
  if (!live) return null;
  const people = live.participants ?? [];
  const notIn = live.not_in ?? [];
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-s-3 overflow-y-auto p-s-4">
      <div className="flex items-center justify-between">
        <h3 className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{`In the call — ${people.length}`}</h3>
        {live.started_ms ? <span className="font-mono text-meta text-muted-foreground">{clockLabel(live.started_ms)}</span> : null}
      </div>
      <ul aria-label="In the call" className="flex flex-col gap-s-2">
        {people.map((p) => (
          <RosterRow key={p.root_id} name={p.display_name} seed={p.root_id} detail={stateOf(p)} />
        ))}
      </ul>
      {notIn.length ? (
        <>
          <h4 className="mt-s-3 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{`Not in the call — ${notIn.length}`}</h4>
          <ul aria-label="Not in the call" className="flex flex-col gap-s-2">
            {notIn.map((p) => (
              <RosterRow key={p.root_id} name={p.display_name} seed={p.root_id} />
            ))}
          </ul>
        </>
      ) : null}
    </div>
  );
}

// What a person in the call is doing, in the words the current UI uses.
function stateOf(p: LiveParticipant): string {
  if (p.forwarder) return p.speaking ? 'Speaking · forwarding the call' : 'Forwarding the call';
  if (p.speaking) return 'Speaking';
  if (p.muted) return 'Mic off';
  return 'Listening';
}

function RosterRow({ name, seed, detail }: { name: string; seed: string; detail?: string }) {
  return (
    <li className="flex items-center gap-s-3">
      <UserAvatar name={name} seed={seed} size="sm" />
      <span className="flex min-w-0 flex-col">
        <span className="font-semibold text-ui text-text-strong">{name}</span>
        {detail ? <span className="text-meta text-muted-foreground">{detail}</span> : null}
      </span>
    </li>
  );
}
