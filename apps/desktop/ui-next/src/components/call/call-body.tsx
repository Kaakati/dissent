// The call pane (the old UI's views/call.js): the header of the call, and in the main column the route banner, the participants in a
// grid with their speaking rings, the controls, and the call chat below. The camera and screen share are disabled slots
// until the video batch lands. The right column shows the call's route, its quality and its encryption (callDetailsNodes).
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { Headphones, Lock, Mic, MicOff, Monitor, MessageCircle, PhoneOff, ShieldCheck, Video, VideoOff, Volume2 } from 'lucide-react';
import { HeaderSearch, MoreTools } from '@/components/chat/chat-header';
import { Composer } from '@/components/chat/composer';
import { listKey, rowsFor } from '@/components/chat/message-list';
import { buildView, type ChatView } from '@/components/chat/use-chat-view';
import '@/components/chat/chat.css';
import { UpdateButton } from '@/components/settings/update-dialog';
import { UserAvatar } from '@/components/user-avatar';
import { useCallRoom } from '@/lib/calls';
import { callConversationOf, chatTargetOf, conversationKey, loadCallChat, useCallChat } from '@/lib/call-chat';
import { api } from '@/lib/api';
import { hueOf } from '@/lib/identity';
import { useShell } from '@/lib/store';
import type { CallState, Id, ShareSource, VoiceParticipant } from '@/lib/types';
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

const NEAR_BOTTOM_PX = 80; // within this distance of the end, a new message keeps the call chat at the end

// The call pane. Replaces the chat while the call is open here.
export function CallBody() {
  const call = useShell((s) => s.call);
  if (!call) return null;
  return (
    <div className="grid min-h-0 min-w-0 grid-rows-[minmax(0,1fr)_400px] gap-s-2 p-s-2">
      <CallStage call={call} />
      <CallChat />
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
        <ShareButton sharing={Boolean(call.screen)} />
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
// A preview's base64 JPEG as a bitmap. The bytes go in a Blob, because the window's img-src policy allows blob: but not data:,
// and createImageBitmap decodes off the main thread.
async function previewBitmap(jpeg: string): Promise<ImageBitmap> {
  const bin = atob(jpeg);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return createImageBitmap(new Blob([bytes], { type: 'image/jpeg' }));
}

// Draws each new preview onto a canvas at the picture's own size; the canvas is scaled to the tile by CSS.
function VideoImage({ preview, label }: { preview: { jpeg: string }; label: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    let live = true;
    void previewBitmap(preview.jpeg)
      .then((bitmap) => {
        const canvas = ref.current;
        if (!live || !canvas) {
          bitmap.close();
          return;
        }
        canvas.width = bitmap.width;
        canvas.height = bitmap.height;
        canvas.getContext('2d')?.drawImage(bitmap, 0, 0);
        bitmap.close();
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [preview.jpeg]);
  return <canvas ref={ref} role="img" aria-label={label} className="absolute inset-0 size-full object-contain" />;
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

// Tells the engine how wide the tile is drawn, so its preview is made that wide. Reported again on resize.
function useTileWidth(ref: React.RefObject<HTMLElement | null>, rootId: string, screen: boolean) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => void api.setVideoTile(rootId, screen, Math.round(el.clientWidth * window.devicePixelRatio)).catch(() => {});
    report();
    const observer = new ResizeObserver(report);
    observer.observe(el);
    return () => {
      observer.disconnect();
      void api.setVideoTile(rootId, screen, 0).catch(() => {});
    };
  }, [ref, rootId, screen]);
}

function Tile({ person }: { person: VoiceParticipant }) {
  const camera = useShell((s) => s.videoFrames[person.root_id]);
  const screen = useShell((s) => s.videoFrames[`${person.root_id}:screen`]);
  const preview = screen ?? camera;
  const ref = useRef<HTMLDivElement>(null);
  useTileWidth(ref, person.root_id, Boolean(screen));
  return (
    <div
      ref={ref}
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

// The screen share control. Sharing stops with one press. Starting opens a list of the displays and windows to share.
function ShareButton({ sharing }: { sharing: boolean }) {
  const toggleScreenShare = useShell((s) => s.toggleScreenShare);
  const [open, setOpen] = useState(false);
  const [sources, setSources] = useState<ShareSource[]>([]);
  const [failed, setFailed] = useState<string | null>(null);
  if (sharing) {
    return (
      <ControlButton label="Stop sharing screen" pressed onClick={() => void toggleScreenShare()}>
        <Monitor className="size-7" />
      </ControlButton>
    );
  }
  const openPicker = async () => {
    setOpen(true);
    try {
      setSources(await api.listShareSources());
      setFailed(null);
    } catch (e) {
      setSources([]);
      setFailed(String(e));
    }
  };
  return (
    <div className="relative">
      <ControlButton label="Share screen" onClick={() => (open ? setOpen(false) : void openPicker())}>
        <Monitor className="size-7" />
      </ControlButton>
      {open ? (
        <div role="menu" aria-label="Share what" className="absolute bottom-full right-0 mb-s-2 grid min-w-64 gap-s-1 rounded-lg border border-border bg-sidebar p-s-2 shadow-lg">
          {failed ? <p className="px-s-2 py-s-1 text-meta text-muted-foreground">{failed}</p> : null}
          {sources.map((source) => (
            <button
              key={`${source.target.kind}-${source.target.id}`}
              type="button"
              role="menuitem"
              className="rounded-md px-s-2 py-s-1 text-left text-[13px] text-text hover:bg-elevated-hover"
              onClick={() => {
                setOpen(false);
                void toggleScreenShare(source.target);
              }}
            >
              {source.primary ? `${source.name} (primary)` : source.name}
            </button>
          ))}
        </div>
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

// The call chat: the messages of the conversation the call sends into, and its composer. Rows and composer are the chat's
// own (message-list.tsx, message-row.tsx, composer.tsx), so a message reads and acts the same here as in the chat:
// formatting, mentions, link previews, invite cards, files, replies, reactions and the message menu.
function CallChat() {
  const view = useCallChatView();
  const audience = view ? audienceOf(view) : null;
  // The conversation the call sends into: its messages are the call chat's. Read again when the call moves to another one.
  const groupId = useCallRoom((r) => r.groupId);
  const conversation = useShell((s) => conversationKey(callConversationOf(s, groupId)));
  useEffect(() => {
    void loadCallChat();
  }, [conversation]);
  return (
    <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-border bg-sidebar" aria-label="Call chat">
      <div className="flex h-12 shrink-0 items-center gap-s-2 pr-s-2 pl-s-4 shadow-[inset_0_-1px_0_var(--border)]">
        <MessageCircle className="size-[18px] text-muted-foreground" aria-hidden="true" />
        <h3 className="text-ui font-semibold text-text-strong">Call chat</h3>
        {audience ? <span className="min-w-0 truncate text-meta text-muted-foreground" title={audience} data-call-chat-audience>{`· ${audience}`}</span> : null}
      </div>
      <CallMessages view={view} />
      {view ? <Composer key={view.target.key} view={view} /> : null}
    </section>
  );
}

// Who reads the call chat. Its messages go into the call's conversation, so everyone who can read that conversation reads
// them, in the call or not: for a voice channel, anyone who can read the channel.
function audienceOf(view: ChatView): string {
  switch (view.target.kind) {
    case 'channel':
      return `Messages go to #${view.title}. Anyone who can read #${view.title} can read them.`;
    case 'dm':
      return `Messages go to your chat with ${view.title}.`;
    case 'group':
      return `Messages go to ${view.title}. Everyone in the group can read them.`;
  }
}

// The call chat's view: the call's conversation built as the chat builds the open one, with the call's own placeholder.
function useCallChatView(): ChatView | null {
  const shell = useShell(
    useShallow((s) => ({ me: s.me, community: s.community, peerName: s.peerName, group: s.group, conversations: s.conversations })),
  );
  const conversation = useCallChat((s) => s.conversation);
  const channel = useCallChat((s) => s.channel);
  const direct = useCallChat((s) => s.direct);
  return useMemo(() => {
    const view = buildView(shell, chatTargetOf(conversation), channel, direct);
    return view ? { ...view, placeholder: 'Message the call' } : null;
  }, [shell, conversation, channel, direct]);
}

// The call chat's messages, kept at the end as they arrive unless scrolled up. Cards that load after their row (an invite,
// a picture) grow the rows later, so a change of the rows' height keeps the end in view too. The scroll event of a move
// made here can arrive after the rows have grown again, so it does not count as the reader scrolling up.
function CallMessages({ view }: { view: ChatView | null }) {
  const list = useRef<HTMLDivElement>(null);
  const rows = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const placed = useRef<number | null>(null); // the scroll position this component last set
  const items = view?.items ?? [];
  const toEnd = (el: HTMLDivElement) => {
    el.scrollTop = el.scrollHeight;
    placed.current = el.scrollTop;
  };
  useLayoutEffect(() => {
    const el = list.current;
    if (el && atBottom.current) toEnd(el);
  });
  useEffect(() => {
    const el = list.current;
    const inner = rows.current;
    if (!el || !inner) return;
    const observer = new ResizeObserver(() => {
      if (atBottom.current) toEnd(el);
    });
    observer.observe(inner);
    return () => observer.disconnect();
  }, []);
  return (
    <div
      ref={list}
      className="messages min-h-0 flex-1"
      role="log"
      aria-label="Call messages"
      aria-live="polite"
      tabIndex={0}
      onScroll={(e) => {
        const el = e.currentTarget;
        if (placed.current !== null && Math.abs(el.scrollTop - placed.current) < 2) return;
        placed.current = null;
        atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
      }}
      onKeyDown={(e) => listKey(e, list.current, items, view)}
    >
      <div ref={rows}>{view ? rowsFor(items, view, null) : null}</div>
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
