// The call pane (the old UI's views/call.js): the header of the call, and in the main column the route banner, the participants in a
// grid with their speaking rings, the controls, and the call chat beside or below. The camera and screen share are disabled slots
// until the video batch lands. The right column shows the call's route, its quality and its encryption (callDetailsNodes).
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { ChevronDown, ChevronUp, Headphones, Lock, Mic, MicOff, Monitor, MessageCircle, Pause, PhoneOff, ShieldCheck, Video, VideoOff, Volume2, X } from 'lucide-react';
import { HeaderSearch, MoreTools } from '@/components/chat/chat-header';
import { Composer } from '@/components/chat/composer';
import { listKey, rowsFor } from '@/components/chat/message-list';
import { buildView, type ChatView } from '@/components/chat/use-chat-view';
import '@/components/chat/chat.css';
import { UpdateButton } from '@/components/settings/update-dialog';
import { Tip } from '@/components/ui/tip';
import { UserAvatar } from '@/components/user-avatar';
import { useCallRoom } from '@/lib/calls';
import { applyHardwareVideo, hardwareVideoWanted, registerCanvas, resetForCall, useEncodedVideoOn } from '@/lib/webcodecs';
import { callConversationOf, chatTargetOf, conversationKey, loadCallChat, useCallChat } from '@/lib/call-chat';
import { api } from '@/lib/api';
import { hueOf } from '@/lib/identity';
import { useShell } from '@/lib/store';
import type { CallState, Id, ShareSource, VoiceParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';
import './call.css';
import { DmCallSplit } from './dm-call-split';

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
  // Everyone in the call, you included, as the voice roster lists them.
  const count = call ? call.participants.length + 1 : 0;
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
      {/* The spacer pushes the tools, the update button and the search to the right edge, as in the chat header. */}
      <span className="flex-1" aria-hidden="true" />
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
// A call pane at least this wide puts the chat beside the video, as Discord does. A narrower one puts it below.
const SIDE_CHAT_MIN_PANE = 1000;
// Where the call chat's open or closed state is kept, on this device.
const CHAT_OPEN_KEY = 'dissent.callChatOpen';
// The space between tiles, in px. Matches gap-s-3.
const TILE_GAP = 12;

function readChatOpen(): boolean {
  try {
    return localStorage.getItem(CHAT_OPEN_KEY) !== 'false';
  } catch {
    return true;
  }
}

function writeChatOpen(open: boolean): void {
  try {
    localStorage.setItem(CHAT_OPEN_KEY, String(open));
  } catch {
    // A window without storage keeps the choice for the session.
  }
}

// The content size of an element, kept current as it resizes.
function useSize(ref: React.RefObject<HTMLElement | null>): { width: number; height: number } {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setSize((s) => (s.width === width && s.height === height ? s : { width, height }));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}

// The call pane. Replaces the chat while the call is open here. The video takes the space, and the chat sits beside it
// in a wide pane or below it, capped, in a narrow one. The chat can be hidden, and that choice is kept on this device.
export function CallBody() {
  // Starts the encoded path when the window's setting asks for it and this window can decode H.264.
  useEffect(() => {
    resetForCall();
    void applyHardwareVideo(hardwareVideoWanted());
  }, []);
  const call = useShell((s) => s.call);
  const root = useRef<HTMLDivElement>(null);
  const { width } = useSize(root);
  const [chatOpen, setChatOpen] = useState(readChatOpen);
  const toggleChat = useCallback(() => {
    setChatOpen((open) => {
      writeChatOpen(!open);
      return !open;
    });
  }, []);
  if (!call) return null;
  // A direct or group call puts the conversation itself below the stage (dm-call-split.tsx). A voice channel keeps the
  // call chat, since its board is not a text channel.
  if (call.scope !== 'channel') return <DmCallSplit call={call} />;
  const side = width >= SIDE_CHAT_MIN_PANE;
  return (
    <div
      ref={root}
      data-call-layout={side ? 'side' : 'bottom'}
      data-call-chat={chatOpen ? 'open' : 'closed'}
      className={cn('flex min-h-0 min-w-0 gap-s-2 p-s-2', side ? 'flex-row' : 'flex-col')}
    >
      <CallStage call={call} chatOpen={chatOpen} onToggleChat={toggleChat} />
      {chatOpen ? <CallChat side={side} onClose={toggleChat} /> : null}
    </div>
  );
}

// The columns and tile size that fit `count` tiles of 16:9 into `width` by `height` with the largest tiles.
export function fitTiles(count: number, width: number, height: number, gap = TILE_GAP): { cols: number; tileWidth: number; tileHeight: number } {
  let best = { cols: 1, tileWidth: 0, tileHeight: 0 };
  for (let cols = 1; cols <= Math.max(1, count); cols++) {
    const rows = Math.ceil(count / cols);
    const byWidth = (width - gap * (cols - 1)) / cols;
    const byHeight = ((height - gap * (rows - 1)) / rows) * (16 / 9);
    const tileWidth = Math.floor(Math.max(0, Math.min(byWidth, byHeight)));
    if (tileWidth > best.tileWidth) best = { cols, tileWidth, tileHeight: Math.floor((tileWidth * 9) / 16) };
  }
  return best;
}

// The tiles, laid out at 16:9 in the columns that make them largest for their number, centred in the space they have.
// A short last row is centred too.
function TileGrid({ count, children }: { count: number; children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  const { width, height } = useSize(box);
  const fit = fitTiles(count, width, height);
  const rowWidth = fit.cols * fit.tileWidth + (fit.cols - 1) * TILE_GAP;
  return (
    <div ref={box} className="min-h-0 min-w-0 p-s-3" data-call-tiles>
      <div
        className="mx-auto flex h-full flex-wrap content-center justify-center gap-s-3 [&>*]:h-[var(--tile-h)] [&>*]:w-[var(--tile-w)]"
        style={{ maxWidth: rowWidth, '--tile-w': `${fit.tileWidth}px`, '--tile-h': `${fit.tileHeight}px` } as CSSProperties}
      >
        {children}
      </div>
    </div>
  );
}

// The stage of a call: the route, the tiles and the controls. The call-chat button shows when `onToggleChat` is given;
// the collapse button when `onToggleCompact` is. Compact, the stage is one strip: the people in the call and the controls.
export function CallStage({
  call,
  chatOpen = false,
  onToggleChat,
  compact = false,
  onToggleCompact,
}: {
  call: CallState;
  chatOpen?: boolean;
  onToggleChat?: () => void;
  compact?: boolean;
  onToggleCompact?: () => void;
}) {
  const toggleMuted = useShell((s) => s.toggleMuted);
  const toggleDeafened = useShell((s) => s.toggleDeafened);
  const toggleCamera = useShell((s) => s.toggleCamera);
  const leaveCall = useShell((s) => s.leaveCall);
  const me = useShell((s) => s.me);
  const ownPreviews = useShell((s) => (me ? s.videoFrames[me.root_id] : undefined));
  const ownScreen = useShell((s) => (me ? s.videoFrames[`${me.root_id}:screen`] : undefined));
  // You always have a tile, as in Discord: your video while you share it, else your avatar.
  const selfTile = Boolean(me);
  return (
    <section
      className={cn(
        'grid min-h-0 min-w-0 flex-1 overflow-hidden rounded-lg bg-ink',
        compact ? 'grid-cols-[minmax(0,1fr)_auto] items-center gap-s-3 px-s-3 py-s-2' : 'grid-rows-[auto_minmax(0,1fr)_80px]',
      )}
      aria-label="Call"
      data-call-compact={compact ? 'true' : undefined}
    >
      {compact ? (
        <ul className="flex min-w-0 items-center gap-s-2 overflow-hidden" aria-label="In the call">
          {me ? (
            <li title={`${me.display_name} (you)`} className={cn('shrink-0 rounded-full', call.self_speaking && 'ring-2 ring-success')} data-self-tile>
              <UserAvatar name={me.display_name} seed={me.root_id} size="sm" />
            </li>
          ) : null}
          {call.participants.map((p) => (
            <li key={p.root_id} title={p.display_name} className={cn('shrink-0 rounded-full', p.speaking && 'ring-2 ring-success')}>
              <UserAvatar name={p.display_name} seed={p.root_id} size="sm" />
            </li>
          ))}
        </ul>
      ) : (
        <>
          <RouteBanner call={call} />
          <TileGrid count={call.participants.length + (selfTile ? 1 : 0)}>
            {selfTile && me ? (
              <SelfTile
                name={me.display_name}
                rootId={me.root_id}
                preview={call.camera || call.screen ? (call.screen ? ownScreen ?? ownPreviews : ownPreviews) : undefined}
                speaking={Boolean(call.self_speaking)}
                muted={call.muted}
              />
            ) : null}
            {call.participants.map((p) => (
              <Tile key={p.root_id} person={p} />
            ))}
          </TileGrid>
        </>
      )}
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
        {onToggleChat ? (
          <ControlButton label={chatOpen ? 'Hide call chat' : 'Show call chat'} expanded={chatOpen} onClick={onToggleChat}>
            <MessageCircle className="size-7" />
          </ControlButton>
        ) : null}
        {onToggleCompact ? (
          <ControlButton label={compact ? 'Expand the call' : 'Collapse the call'} expanded={!compact} onClick={onToggleCompact}>
            {compact ? <ChevronDown className="size-7" /> : <ChevronUp className="size-7" />}
          </ControlButton>
        ) : null}
        <Tip label="Leave call">
          <button
            type="button"
            aria-label="Leave call"
            onClick={() => void leaveCall()}
            className="grid h-14 w-[72px] place-items-center rounded-[28px] bg-danger text-white hover:bg-danger-hover focus-visible:outline-2 focus-visible:outline-ring"
          >
            <PhoneOff className="size-7" aria-hidden="true" />
          </button>
        </Tip>
      </div>
    </section>
  );
}

// The route sentence the engine writes, with the quality beside it. A forwarded call names its forwarder and backup, or the
// member outside the call who forwards it.
function RouteBanner({ call }: { call: CallState }) {
  const { route, quality } = call;
  const { forwarder, backup, volunteer } = routeParties(call);
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
            {volunteer
              ? 'They aren’t in the call. Their computer passes everyone’s sealed streams along and can’t open them.'
              : `Their computer passes everyone’s sealed streams along and can’t open them. ${backup ?? 'Someone else'} is ready as backup.`}
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
function SelfTile({ name, rootId, preview, speaking, muted }: { name: string; rootId: Id; preview?: { jpeg: string }; speaking: boolean; muted: boolean }) {
  return (
    <div
      data-self-tile
      data-speaking={speaking || undefined}
      className={cn('relative grid place-items-center overflow-hidden rounded-lg', speaking && 'shadow-[inset_0_0_0_3px_var(--success)]')}
      style={{ backgroundColor: `oklch(26% 0.03 ${hueOf(rootId)})` }}
    >
      {preview ? (
        <VideoImage preview={preview} label="Your video" />
      ) : (
        <UserAvatar name={name} seed={rootId} size="xl" speaking={speaking} className="aspect-square h-[min(6rem,55%)] w-auto text-[clamp(11px,2.2vh,31px)]" />
      )}
      <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-[12px] leading-[18px] font-medium text-text-strong">
        {muted ? <MicOff className="size-3.5 text-danger-text" aria-hidden="true" /> : null}
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
  // The forwarder pauses a stream for this device when its bandwidth cannot fit it. The last frame stays, marked as paused.
  const paused = screen ? person.screen_paused : person.video_paused;
  const encoded = useEncodedVideoOn(person.root_id);
  // Stable callbacks, so a re-render does not detach the canvases: a canvas is released only when its tile goes.
  const cameraRef = useCallback((el: HTMLCanvasElement | null) => registerCanvas(person.root_id, el), [person.root_id]);
  const screenRef = useCallback((el: HTMLCanvasElement | null) => registerCanvas(`${person.root_id}:screen`, el), [person.root_id]);
  const ref = useRef<HTMLDivElement>(null);
  useTileWidth(ref, person.root_id, Boolean(screen));
  return (
    <div
      ref={ref}
      data-tile={person.root_id}
      data-speaking={person.speaking || undefined}
      className={cn('relative grid place-items-center overflow-hidden rounded-lg', person.speaking && 'shadow-[inset_0_0_0_3px_var(--success)]')}
      style={{ backgroundColor: `oklch(26% 0.03 ${hueOf(person.root_id)})` }}
    >
      {encoded ? (
        // Decoded here: the camera, then the screen on top of it, which stays hidden until its first frame.
        <>
          <canvas
            ref={cameraRef}
            aria-label={`${person.display_name}'s video`}
            hidden
            className="absolute inset-0 size-full object-contain"
          />
          <canvas
            ref={screenRef}
            aria-label={`${person.display_name}'s screen`}
            hidden
            className="absolute inset-0 size-full object-contain"
          />
        </>
      ) : null}
      {encoded || preview ? null : <UserAvatar name={person.display_name} seed={person.root_id} size="xl" speaking={person.speaking} className="aspect-square h-[min(6rem,55%)] w-auto text-[clamp(11px,2.2vh,31px)]" />}
      {encoded || !preview ? null : <VideoImage preview={preview} label={`${person.display_name}'s video`} />}
      <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-[12px] leading-[18px] font-medium text-text-strong">
        {person.muted ? <MicOff className="size-3.5 text-danger-text" aria-hidden="true" /> : null}
        <span>{person.display_name}</span>
      </span>
      {paused && (preview || encoded) ? (
        <span className="absolute top-2.5 left-2.5 inline-flex h-[26px] items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-meta text-text">
          <Pause className="size-3.5" aria-hidden="true" />
          Paused to save bandwidth
        </span>
      ) : null}
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
  expanded,
  disabled,
  onClick,
  children,
}: {
  label: string;
  pressed?: boolean;
  expanded?: boolean; // for a button that shows or hides a panel
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  return (
    <Tip label={disabled ? 'Coming soon' : label} wrap={disabled}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={pressed}
        aria-expanded={expanded}
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
    </Tip>
  );
}

// The call chat: the messages of the conversation the call sends into, and its composer. Rows and composer are the chat's
// own (message-list.tsx, message-row.tsx, composer.tsx), so a message reads and acts the same here as in the chat:
// formatting, mentions, link previews, invite cards, files, replies, reactions and the message menu.
function CallChat({ side, onClose }: { side: boolean; onClose: () => void }) {
  const view = useCallChatView();
  const audience = view ? audienceOf(view) : null;
  // The conversation the call sends into: its messages are the call chat's. Read again when the call moves to another one.
  const groupId = useCallRoom((r) => r.groupId);
  const conversation = useShell((s) => conversationKey(callConversationOf(s, groupId)));
  useEffect(() => {
    void loadCallChat();
  }, [conversation]);
  return (
    <section
      className={cn(
        'flex min-h-0 shrink-0 flex-col overflow-hidden rounded-lg border border-border bg-sidebar',
        // Beside the video it is a column; below it, it takes at most a third of the pane, so the video keeps the rest.
        side ? 'w-[360px]' : 'h-[min(33%,300px)] min-h-[200px]',
      )}
      aria-label="Call chat"
    >
      <div className="flex h-12 shrink-0 items-center gap-s-2 pr-s-2 pl-s-4 shadow-[inset_0_-1px_0_var(--border)]">
        <MessageCircle className="size-[18px] shrink-0 text-muted-foreground" aria-hidden="true" />
        <h3 className="shrink-0 text-ui font-semibold text-text-strong">Call chat</h3>
        {audience ? <span className="min-w-0 flex-1 truncate text-meta text-muted-foreground" title={audience} data-call-chat-audience>{`· ${audience}`}</span> : <span className="flex-1" />}
        <Tip label="Hide call chat">
          <button
            type="button"
            aria-label="Hide call chat"
            onClick={onClose}
            className="grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-elevated-hover hover:text-text-strong focus-visible:outline-2 focus-visible:outline-ring"
          >
            <X className="size-4" aria-hidden="true" />
          </button>
        </Tip>
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
  const { forwarder, backup, volunteer } = routeParties(call);
  const quality = call.quality;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between gap-s-2 px-s-4 pr-s-2 text-sm font-semibold text-text-strong shadow-[inset_0_-1px_0_var(--border)]">
        <h3 className="truncate">Call details</h3>
      </div>
      <div className="dissent-scroll flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-s-2 pb-s-2">
        {forwarder ? (
          <Section
            title="Forwarded by"
            text={volunteer ? `${forwarder} · not in the call, helping forward this server’s calls` : `${forwarder} · chosen automatically for a strong, wired connection`}
          />
        ) : null}
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

// The forwarder and the backup of a call: a member outside the call who forwards it (`volunteer`), named by the route, or
// the participant the engine flags as forwarding.
function routeParties(call: CallState): { forwarder: string | null; backup: string | null; volunteer: boolean } {
  if (call.forwarded_by) return { forwarder: call.forwarded_by, backup: null, volunteer: true };
  const flagged = call.participants.find((p) => p.forwarder);
  const forwarder = call.route?.forwarder ?? (flagged ? flagged.display_name : null);
  return { forwarder: forwarder || null, backup: call.route?.backup || null, volunteer: false };
}

function Section({ title, text }: { title: string; text: string }) {
  return (
    <section className="px-s-4 pt-s-3 pb-s-3 shadow-[inset_0_-1px_0_var(--border)]">
      <h4 className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{title}</h4>
      <p className="mt-s-1 text-sm leading-[19px] text-text">{text}</p>
    </section>
  );
}
