// The call pane (the old UI's views/call.js): the header of the call, and in the main column the route banner, the participants in a
// grid with their speaking rings, and the controls; a direct or group call's conversation below. The camera and screen share are disabled slots
// until the video batch lands. The right column shows the call's route, its quality and its encryption (callDetailsNodes).
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { ChevronDown, ChevronUp, Headphones, Lock, Mic, MicOff, Monitor, MonitorOff, Pause, PhoneOff, Settings2, ShieldCheck, TriangleAlert, Video, VideoOff, Volume2 } from 'lucide-react';
import { HeaderSearch, MoreTools } from '@/components/chat/chat-header';
import '@/components/chat/chat.css';
import { UpdateButton } from '@/components/settings/update-dialog';
import { Tip } from '@/components/ui/tip';
import { UserAvatar } from '@/components/user-avatar';
import { applyHardwareVideo, hardwareVideoWanted, registerCanvas, resetForCall, useEncodedVideoOn } from '@/lib/webcodecs';
import { api } from '@/lib/api';
import { hueOf } from '@/lib/identity';
import { useShell } from '@/lib/store';
import type { CallState, Id, VoiceParticipant } from '@/lib/types';
import { cn } from '@/lib/utils';
import './call.css';
import { DmCallSplit } from './dm-call-split';
import { ScreenStage, type Sharer } from './screen-stage';
import { openShareDialog } from './share-dialog';

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
        <span className="title-meta">{call ? `${count} in call` : 'Joining'}</span>
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

// The space between tiles, in px. Matches gap-s-3.
const TILE_GAP = 12;

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

// The call pane. Replaces the chat while the call is open here. A voice channel's stage fills the pane: the channel's
// text is not shown beside it, so its board is the call alone. A direct or group call puts its conversation below the
// stage (dm-call-split.tsx). Until the engine reports the call, a join shows that it is on its way.
export function CallBody() {
  // Starts the encoded path when the window's setting asks for it and this window can decode H.264.
  useEffect(() => {
    resetForCall();
    void applyHardwareVideo(hardwareVideoWanted());
  }, []);
  const call = useShell((s) => s.call);
  const joining = useShell((s) => Boolean(s.voiceConnection?.joining));
  if (!call) return joining ? <CallJoining /> : null;
  if (call.scope !== 'channel') return <DmCallSplit call={call} />;
  return (
    <div data-call-layout="stage" className="flex min-h-0 min-w-0 p-s-2">
      <CallStage call={call} />
    </div>
  );
}

// The board between the click on a voice channel and the engine's call: the channel is joining.
function CallJoining() {
  const name = useCallName();
  return (
    <section aria-label="Call" data-call-joining className="flex min-h-0 min-w-0 flex-col items-center justify-center gap-s-2 p-s-8 text-center">
      <Volume2 className="size-10 text-faint" aria-hidden="true" />
      <p className="text-body text-muted-foreground">{name ? `Joining #${name}` : 'Joining the voice channel'}</p>
    </section>
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

// The stage of a call: the route, the tiles and the controls. The collapse button shows when `onToggleCompact` is given.
// Compact, the stage is one strip: the people in the call and the controls.
export function CallStage({
  call,
  compact = false,
  onToggleCompact,
}: {
  call: CallState;
  compact?: boolean;
  onToggleCompact?: () => void;
}) {
  const toggleMuted = useShell((s) => s.toggleMuted);
  const toggleDeafened = useShell((s) => s.toggleDeafened);
  const toggleCamera = useShell((s) => s.toggleCamera);
  const toggleScreenShare = useShell((s) => s.toggleScreenShare);
  const leaveCall = useShell((s) => s.leaveCall);
  const me = useShell((s) => s.me);
  const ownPreviews = useShell((s) => (me ? s.videoFrames[me.root_id] : undefined));
  const name = useCallName();
  // You always have a tile, as in Discord: your camera while it is on, else your avatar.
  const selfTile = Boolean(me);
  // Whoever shares a screen, others first, so a viewer's stage shows what someone else is sharing before their own.
  const sharers: Sharer[] = call.participants.filter((p) => p.screen).map((p) => ({ rootId: p.root_id, name: p.display_name, self: false, sound: p.screen_audio }));
  if (call.screen && me) sharers.push({ rootId: me.root_id, name: me.display_name, self: true });
  const tiles = (
    <>
      {selfTile && me ? (
        <SelfTile
          name={me.display_name}
          rootId={me.root_id}
          preview={call.camera ? ownPreviews : undefined}
          speaking={Boolean(call.self_speaking)}
          muted={call.muted}
          live={Boolean(call.screen)}
        />
      ) : null}
      {call.participants.map((p) => (
        <Tile key={p.root_id} person={p} />
      ))}
    </>
  );
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
          {/* The screen needs the room, and the route is still in the call's details. A call that cannot decrypt says so anyway. */}
          {sharers.length && !call.cannot_decrypt ? <span /> : <RouteBanner call={call} />}
          {sharers.length ? (
            <ScreenStage sharers={sharers} people={tiles} />
          ) : (
            <TileGrid count={call.participants.length + (selfTile ? 1 : 0)}>{tiles}</TileGrid>
          )}
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
        {call.screen ? (
          <>
            <ControlButton label="Stream settings" onClick={() => openShareDialog(name, true)}>
              <Settings2 className="size-7" />
            </ControlButton>
            <Tip label="Stop streaming">
              <button
                type="button"
                aria-label="Stop streaming"
                onClick={() => void toggleScreenShare()}
                className="grid size-14 place-items-center rounded-full bg-danger text-white hover:bg-danger-hover focus-visible:outline-2 focus-visible:outline-ring"
              >
                <MonitorOff className="size-7" aria-hidden="true" />
              </button>
            </Tip>
          </>
        ) : (
          <ControlButton label="Share screen" onClick={() => openShareDialog(name, false)}>
            <Monitor className="size-7" />
          </ControlButton>
        )}
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
  if (call.cannot_decrypt) {
    return (
      <div role="alert" data-call-cannot-decrypt className="mx-s-3 mt-s-3 flex items-center gap-s-3 rounded-lg bg-sidebar px-s-4 py-2.5 text-ui text-danger-text">
        <TriangleAlert className="size-5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 flex-1">You can’t hear this call: its audio is arriving, and this device can’t decrypt it. Try leaving and joining again.</span>
      </div>
    );
  }
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
export function VideoImage({ preview, label }: { preview: { jpeg: string }; label: string }) {
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

// This device's own tile: the camera, or the avatar. A shared screen has the stage to itself.
function SelfTile({ name, rootId, preview, speaking, muted, live }: { name: string; rootId: Id; preview?: { jpeg: string }; speaking: boolean; muted: boolean; live: boolean }) {
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
      {live ? <span data-tile-live className="absolute top-2.5 right-2.5 inline-flex h-5 items-center rounded bg-danger px-1.5 text-[11px] font-bold tracking-wide text-white">LIVE</span> : null}
    </div>
  );
}

// Tells the engine how wide the tile is drawn, so its preview is made that wide. Reported again on resize.
export function useTileWidth(ref: React.RefObject<HTMLElement | null>, rootId: string, screen: boolean, skip = false) {
  useEffect(() => {
    const el = ref.current;
    if (!el || skip) return;
    const report = () => void api.setVideoTile(rootId, screen, Math.round(el.clientWidth * window.devicePixelRatio)).catch(() => {});
    report();
    const observer = new ResizeObserver(report);
    observer.observe(el);
    return () => {
      observer.disconnect();
      void api.setVideoTile(rootId, screen, 0).catch(() => {});
    };
  }, [ref, rootId, screen, skip]);
}

function Tile({ person }: { person: VoiceParticipant }) {
  const preview = useShell((s) => s.videoFrames[person.root_id]);
  // The forwarder pauses a stream for this device when its bandwidth cannot fit it. The last frame stays, marked as paused.
  const paused = person.video_paused;
  const encoded = useEncodedVideoOn(person.root_id);
  // Stable callbacks, so a re-render does not detach the canvases: a canvas is released only when its tile goes.
  const cameraRef = useCallback((el: HTMLCanvasElement | null) => registerCanvas(person.root_id, el), [person.root_id]);
  const ref = useRef<HTMLDivElement>(null);
  useTileWidth(ref, person.root_id, false);
  return (
    <div
      ref={ref}
      data-tile={person.root_id}
      data-speaking={person.speaking || undefined}
      className={cn('relative grid place-items-center overflow-hidden rounded-lg', person.speaking && 'shadow-[inset_0_0_0_3px_var(--success)]')}
      style={{ backgroundColor: `oklch(26% 0.03 ${hueOf(person.root_id)})` }}
    >
      {encoded ? (
        // Decoded here. The camera stays hidden until its first frame, and a shared screen has its own tile on the stage.
        <canvas ref={cameraRef} aria-label={`${person.display_name}'s video`} hidden className="absolute inset-0 size-full object-contain" />
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
      {person.screen ? <span data-tile-live className={cn('absolute right-2.5 inline-flex h-5 items-center rounded bg-danger px-1.5 text-[11px] font-bold tracking-wide text-white', person.forwarder ? 'top-10' : 'top-2.5')}>LIVE</span> : null}
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
