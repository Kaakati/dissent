// The call pane (the old UI's views/call.js): the header of the call, and in the main column the route banner, the participants in a
// grid with their speaking rings, the controls, and the call chat below. The camera and screen share are disabled slots
// until the video batch lands. The right column shows the call's route, its quality and its encryption (callDetailsNodes).
import { useEffect, useState, type ReactNode } from 'react';
import { Headphones, Lock, Mic, MicOff, Monitor, MessageCircle, PhoneOff, Send, ShieldCheck, Video, VideoOff, Volume2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { HeaderSearch, MoreTools } from '@/components/chat/chat-header';
import { UpdateButton } from '@/components/settings/update-dialog';
import { UserAvatar } from '@/components/user-avatar';
import { sendCallMessage, useCallRoom } from '@/lib/calls';
import { callConversationOf, conversationKey, loadCallChat, useCallChat } from '@/lib/call-chat';
import { hueOf } from '@/lib/identity';
import { clockLabel } from '@/lib/time';
import { useShell } from '@/lib/store';
import type { CallMessage, CallState, Id, VoiceParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';
import './call.css';

// The name of the open call: the voice channel, or the person or group of a direct or group call.
function useCallName(): string {
  return useShell((s) => {
    if (s.route.kind === 'server') return s.community?.channels.find((c) => c.id === s.voiceChannelId)?.name ?? s.call?.channel_name ?? '';
    return s.group?.name ?? s.peerName;
  });
}

// The call's header: the name, the count and the end-to-end pill (the chat header's place).
export function CallHeader() {
  const call = useShell((s) => s.call);
  const name = useCallName();
  const count = call?.participants.length ?? 0;
  return (
    <div className="chat-header">
      <div className="chat-title">
        <Volume2 aria-hidden="true" />
        <h2 title={name}>{name}</h2>
        <span className="title-meta">{`${count} in call`}</span>
        <span className="pill ok">
          <Lock aria-hidden="true" />
          End-to-end encrypted
        </span>
      </div>
      <div className="header-tools">
        <MoreTools />
      </div>
      <div className="update-host">
        <UpdateButton />
      </div>
      <HeaderSearch />
    </div>
  );
}

// The call pane. Replaces the chat while the call is open here.
export function CallBody() {
  const call = useShell((s) => s.call);
  if (!call) return null;
  return (
    <div className="grid min-h-0 min-w-0 grid-rows-[minmax(0,1fr)_400px] gap-s-2 p-s-2">
      <CallStage call={call} />
      <CallChat call={call} />
    </div>
  );
}

function CallStage({ call }: { call: CallState }) {
  const toggleMuted = useShell((s) => s.toggleMuted);
  const toggleDeafened = useShell((s) => s.toggleDeafened);
  const toggleCamera = useShell((s) => s.toggleCamera);
  const leaveCall = useShell((s) => s.leaveCall);
  const me = useShell((s) => s.me);
  const ownPreviews = useShell((s) => (me ? s.videoFrames[me.root_id] : undefined));
  const ownScreen = useShell((s) => (me ? s.videoFrames[`${me.root_id}:screen`] : undefined));
  return (
    <section className="grid min-h-0 min-w-0 grid-rows-[auto_minmax(0,1fr)_96px] overflow-hidden rounded-lg bg-ink" aria-label="Call">
      <RouteBanner call={call} />
      <div className="grid min-h-0 grid-cols-3 auto-rows-[minmax(0,1fr)] gap-s-3 p-s-3">
        {me && (call.camera || call.screen) ? (
          <SelfTile name={me.display_name} rootId={me.root_id} preview={call.screen ? ownScreen ?? ownPreviews : ownPreviews} />
        ) : null}
        {call.participants.map((p) => (
          <Tile key={p.root_id} person={p} />
        ))}
      </div>
      <div className="flex items-center justify-center gap-s-3">
        <ControlButton
          label={call.muted ? 'Unmute microphone' : 'Mute microphone'}
          pressed={call.muted}
          onClick={() => void toggleMuted()}
        >
          {call.muted ? <MicOff className="size-7" /> : <Mic className="size-7" />}
        </ControlButton>
        <ControlButton
          label={call.deafened ? 'Undeafen' : 'Deafen'}
          pressed={call.deafened}
          onClick={() => void toggleDeafened()}
        >
          <Headphones className="size-7" />
        </ControlButton>
        <ControlButton
          label={call.camera ? 'Turn camera off' : 'Turn camera on'}
          pressed={Boolean(call.camera)}
          onClick={() => void toggleCamera()}
        >
          {call.camera ? <Video className="size-7" /> : <VideoOff className="size-7" />}
        </ControlButton>
        <ControlButton label="Share screen, coming soon" disabled>
          <Monitor className="size-7" />
        </ControlButton>
        <button
          type="button"
          aria-label="Leave call"
          title="Leave call"
          onClick={() => void leaveCall()}
          className="grid h-14 w-[72px] place-items-center rounded-[28px] bg-danger text-white hover:bg-danger-hover focus-visible:outline-2 focus-visible:outline-ring"
        >
          <PhoneOff className="size-7" aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}

// The route sentence the engine writes, with the quality beside it. A forwarded call names its forwarder and backup.
function RouteBanner({ call }: { call: CallState }) {
  const { route, quality } = call;
  const { forwarder, backup } = routeParties(call);
  const forwarded = forwarder !== null;
  const reading = quality
    ? `${quality.rtt_ms === null ? '—' : Math.round(quality.rtt_ms)} ms · ${quality.loss_pct.toFixed(1)}% loss · ${Math.round(quality.jitter_ms)} ms jitter`
    : null;
  return (
    <div role="status" className="mx-s-3 mt-s-3 flex items-center gap-s-3 rounded-lg bg-sidebar px-s-4 py-2.5 text-ui">
      <ShieldCheck className="size-5 shrink-0 text-accent-text" aria-hidden="true" />
      <span className="min-w-0 flex-1 text-muted-foreground">
        {forwarded ? (
          <>
            <strong className="font-semibold text-text-strong">{`Forwarded by ${forwarder}.`}</strong>{' '}
            {`Their computer passes everyone’s sealed streams along and can’t open them. ${backup ?? 'Someone else'} is ready as backup.`}
          </>
        ) : (
          <>
            <strong className="font-semibold text-text-strong">{route?.title ?? 'In a call'}</strong>
            {route?.detail ? ` ${route.detail}` : null}
          </>
        )}
      </span>
      {reading ? <span className="shrink-0 font-mono text-meta text-muted-foreground">{reading}</span> : null}
    </div>
  );
}

// One participant: a large avatar on the hue of their id, with the speaking ring, the mute flag and the forwarder tag.
// A person's camera or screen, drawn from the latest preview the engine sent: a JPEG, base64 encoded.
function VideoImage({ preview, label }: { preview: { jpeg: string }; label: string }) {
  return <img src={`data:image/jpeg;base64,${preview.jpeg}`} alt={label} className="absolute inset-0 size-full object-contain" />;
}

// This device's own tile. It shows the shared screen when there is one, else the camera.
function SelfTile({ name, rootId, preview }: { name: string; rootId: Id; preview?: { jpeg: string } }) {
  return (
    <div className="relative grid place-items-center overflow-hidden rounded-lg" style={{ backgroundColor: `oklch(26% 0.03 ${hueOf(rootId)})` }}>
      {preview ? <VideoImage preview={preview} label="Your video" /> : <UserAvatar name={name} seed={rootId} size="xl" className="size-24 text-[31px]" />}
      <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center rounded-md bg-black/55 px-2.5 text-[12px] leading-[18px] font-medium text-text-strong">
        {`${name} (you)`}
      </span>
    </div>
  );
}

function Tile({ person }: { person: VoiceParticipant }) {
  const camera = useShell((s) => s.videoFrames[person.root_id]);
  const screen = useShell((s) => s.videoFrames[`${person.root_id}:screen`]);
  const preview = screen ?? camera;
  return (
    <div
      className={cn('relative grid place-items-center overflow-hidden rounded-lg', person.speaking && 'shadow-[inset_0_0_0_3px_var(--success)]')}
      style={{ backgroundColor: `oklch(26% 0.03 ${hueOf(person.root_id)})` }}
    >
      {preview ? <VideoImage preview={preview} label={`${person.display_name}'s video`} /> : null}
      {preview ? null : <UserAvatar name={person.display_name} seed={person.root_id} size="xl" className="size-24 text-[31px]" />}
      <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-[12px] leading-[18px] font-medium text-text-strong">
        {person.muted ? <MicOff className="size-3.5 text-danger-text" aria-hidden="true" /> : null}
        <span>{person.display_name}</span>
      </span>
      {person.forwarder ? (
        <span className="absolute top-2.5 right-2.5 inline-flex h-[26px] items-center rounded-md bg-black/55 px-2.5 text-meta text-text">Forwarding</span>
      ) : null}
    </div>
  );
}

// A round control of the call. A disabled one is a slot for a feature that is not built yet.
function ControlButton({
  label,
  pressed,
  disabled,
  onClick,
  children,
}: {
  label: string;
  pressed?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={pressed}
      title={disabled ? 'Coming soon' : label}
      aria-disabled={disabled || undefined}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'grid size-14 place-items-center rounded-full bg-elevated text-text-strong hover:bg-elevated-hover focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50',
        pressed && 'bg-danger-tint text-danger-text',
      )}
    >
      {children}
    </button>
  );
}

// The call chat: the messages of the call, and a composer that sends into the call's conversation.
function CallChat({ call }: { call: CallState }) {
  const [text, setText] = useState('');
  const count = call.participants.length;
  const messages = useCallChat((s) => s.messages);
  // The conversation the call sends into: its messages are the call chat's. Read again when the call moves to another one.
  const groupId = useCallRoom((r) => r.groupId);
  const conversation = useShell((s) => conversationKey(callConversationOf(s, groupId)));
  useEffect(() => {
    void loadCallChat();
  }, [conversation]);
  const send = async () => {
    const line = text.trim();
    if (!line) return;
    if (await sendCallMessage(line)) {
      setText('');
      void loadCallChat();
    }
  };
  return (
    <section className="grid min-h-0 grid-rows-[48px_minmax(0,1fr)_auto] overflow-hidden rounded-lg border border-border bg-sidebar" aria-label="Call chat">
      <div className="flex items-center gap-s-2 pr-s-2 pl-s-4 shadow-[inset_0_-1px_0_var(--border)]">
        <MessageCircle className="size-[18px] text-muted-foreground" aria-hidden="true" />
        <h3 className="text-ui font-semibold text-text-strong">Call chat</h3>
        <span className="text-meta text-muted-foreground">{`· Only the ${count} people in this call can read it`}</span>
      </div>
      <div className="dissent-scroll flex flex-col justify-end overflow-y-auto py-s-1 pb-s-2">
        {messages.map((m) => (
          <CallMessageRow key={m.id} message={m} />
        ))}
      </div>
      <form
        className="m-s-2 flex min-h-11 items-center rounded-lg border border-input-border bg-input pr-1.5 pl-s-2"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Message the call"
          aria-label="Message the call"
          className="h-10 min-w-0 flex-1 border-0 bg-transparent px-s-2 text-ui text-text-strong outline-none placeholder:text-faint"
        />
        <Button variant="ghost" size="icon" type="submit" aria-label="Send to the call" title="Send to the call" className="text-accent-text hover:bg-accent hover:text-white">
          <Send className="size-4" />
        </Button>
      </form>
    </section>
  );
}

function CallMessageRow({ message }: { message: CallMessage }) {
  return (
    <div className="mt-s-3 grid grid-cols-[56px_minmax(0,1fr)] pr-s-4">
      <div className="relative">
        <UserAvatar name={message.author_name} seed={message.root_id} size="sm" className="absolute top-0.5 left-s-4 size-7 text-[10px]" />
      </div>
      <div className="min-w-0">
        <div className="flex items-baseline gap-1.5">
          <span className="text-sm leading-[19px] font-medium" style={{ color: `oklch(80% 0.12 ${hueOf(message.root_id)})` }}>{message.author_name}</span>
          <time className="text-tiny text-muted-foreground">{clockLabel(Number(message.ms))}</time>
        </div>
        {message.text ? <div className="text-sm leading-[19px] break-words text-text">{message.text}</div> : null}
        {message.file ? (
          <div className="mt-s-1 flex w-[340px] max-w-full items-center gap-2.5 rounded-lg border border-border bg-panel px-2.5 py-s-2">
            <span className="grid h-9 w-[30px] place-items-center rounded-[4px] bg-elevated text-[8px] font-bold text-text-strong">{message.file.kind}</span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[12px] leading-[18px] font-semibold text-link">{message.file.name}</span>
              <span className="block text-meta text-success-text">{message.file.meta}</span>
            </span>
          </div>
        ) : null}
      </div>
    </div>
  );
}

// The right column during a channel call: who forwards the call and who is the backup, when the engine names them, the
// quality, and the encryption (the old UI's views/call.js callDetailsNodes). Each section is there only when its field is.
export function CallDetails() {
  const call = useShell((s) => s.call);
  if (!call) return null;
  const { forwarder, backup } = routeParties(call);
  const quality = call.quality;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between gap-s-2 px-s-4 pr-s-2 text-sm font-semibold text-text-strong shadow-[inset_0_-1px_0_var(--border)]">
        <h3 className="truncate">Call details</h3>
      </div>
      <div className="dissent-scroll flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-s-2 pb-s-2">
        {forwarder ? <Section title="Forwarded by" text={`${forwarder} · chosen automatically for a strong, wired connection`} /> : null}
        {backup ? <Section title="Backup" text={`${backup} · connected and ready to take over`} /> : null}
        {quality ? (
          <Section
            title="Quality"
            text={`${quality.rtt_ms === null ? 'No round trip yet' : `${Math.round(quality.rtt_ms)} ms round trip`} · ${quality.loss_pct.toFixed(1)}% loss · ${Math.round(quality.jitter_ms)} ms jitter${quality.flagged ? ` · ${quality.flagged} frames flagged` : ''}`}
          />
        ) : null}
        <Section title="Encryption" text="Streams are sealed on each device. A forwarder passes them on and can’t open them." />
        {forwarder ? <Section title="What the forwarder sees" text="Who is speaking and how loudly, so it can pick the active speakers. Never what is said." /> : null}
      </div>
    </div>
  );
}

// The forwarder and the backup of a call: named by the route, or the participant the engine flags as forwarding.
function routeParties(call: CallState): { forwarder: string | null; backup: string | null } {
  const flagged = call.participants.find((p) => p.forwarder);
  const forwarder = call.route?.forwarder ?? (flagged ? flagged.display_name : null);
  return { forwarder: forwarder || null, backup: call.route?.backup || null };
}

function Section({ title, text }: { title: string; text: string }) {
  return (
    <section className="px-s-4 pt-s-3 pb-s-3 shadow-[inset_0_-1px_0_var(--border)]">
      <h4 className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{title}</h4>
      <p className="mt-s-1 text-sm leading-[19px] text-text">{text}</p>
    </section>
  );
}
