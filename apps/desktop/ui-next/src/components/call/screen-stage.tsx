// A shared screen on the call's stage: whole, on black, with everyone else as a strip below it. The first sharer takes the stage
// by themselves; with several, a viewer picks from the strip. It goes full screen with a button or a double-click, and Escape
// comes back. Only this block is shown full screen, so the strip goes along and the call's controls do not.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { ChevronDown, ChevronUp, Maximize, Minimize, Monitor, Pin, PinOff } from 'lucide-react';
import { Tip } from '@/components/ui/tip';
import { registerCanvas, useEncodedVideoOn } from '@/lib/webcodecs';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';
import { useTileWidth, VideoImage } from './call-body';

// A stage shorter than this gives the strip's room to the screen.
const STRIP_MIN_STAGE = 280;

export interface Sharer {
  rootId: string;
  name: string;
  self: boolean;
}

// The app window's own state, which the page cannot set by itself. Nothing happens outside the app.
function setWindow(command: 'set_fullscreen' | 'set_always_on_top', value: boolean) {
  void window.__TAURI__?.core.invoke(`plugin:window|${command}`, { label: 'main', value })?.catch(() => {});
}

// The picture of one shared screen, always whole. A viewer's window decodes it onto a canvas, or draws the engine's previews.
function ScreenPicture({ sharer, onSize }: { sharer: Sharer; onSize?: (height: number) => void }) {
  const key = `${sharer.rootId}:screen`;
  const preview = useShell((s) => s.videoFrames[key]);
  const encoded = useEncodedVideoOn(sharer.rootId) && !sharer.self;
  const box = useRef<HTMLDivElement>(null);
  const canvasRef = useCallback((el: HTMLCanvasElement | null) => registerCanvas(key, el), [key]);
  useTileWidth(box, sharer.rootId, true, sharer.self);
  // A decoded canvas is as large as the stream. A preview is only as wide as the tile, so it says nothing about the stream.
  useEffect(() => {
    if (!encoded || !onSize) return;
    const timer = setInterval(() => onSize(box.current?.querySelector('canvas')?.height ?? 0), 1000);
    return () => clearInterval(timer);
  }, [encoded, onSize]);
  return (
    <div ref={box} className="absolute inset-0">
      {encoded ? (
        <canvas ref={canvasRef} aria-label={`${sharer.name}'s screen`} hidden className="absolute inset-0 size-full object-contain" />
      ) : preview ? (
        <VideoImage preview={preview} label={`${sharer.name}'s screen`} />
      ) : null}
    </div>
  );
}

function LiveBadge() {
  return <span className="inline-flex h-5 items-center rounded bg-danger px-1.5 text-[11px] font-bold tracking-wide text-white">LIVE</span>;
}

function StageButton({ label, pressed, onClick, children }: { label: string; pressed?: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <Tip label={label}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={pressed}
        onClick={onClick}
        className={cn('grid size-8 place-items-center rounded-md bg-black/55 text-text-strong hover:bg-black/75 focus-visible:outline-2 focus-visible:outline-ring', pressed && 'text-accent-text')}
      >
        {children}
      </button>
    </Tip>
  );
}

// The stage while someone shares. `people` are the tiles of everyone in the call, the sharers included, for the strip.
export function ScreenStage({ sharers, people }: { sharers: Sharer[]; people: ReactNode }) {
  const [picked, setPicked] = useState<string | null>(null);
  const main = sharers.find((s) => s.rootId === picked) ?? sharers[0];
  const others = sharers.filter((s) => s !== main);
  const [full, setFull] = useState(false);
  const [stripOpen, setStripOpen] = useState(true);
  const [onTop, setOnTop] = useState(false);
  const [height, setHeight] = useState(0);
  const block = useRef<HTMLDivElement>(null);
  const [blockHeight, setBlockHeight] = useState(Infinity);
  useLayoutEffect(() => {
    const el = block.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => setBlockHeight(entry.contentRect.height));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // The full-screen state follows the browser's, which Escape ends by itself.
  const leave = useCallback(() => {
    setFull(false);
    setWindow('set_fullscreen', false);
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
  }, []);
  const enter = useCallback(() => {
    setFull(true);
    setWindow('set_fullscreen', true);
    // A window that refuses keeps the block over the whole window instead, by its fixed class.
    void block.current?.requestFullscreen?.().catch(() => {});
  }, []);
  useEffect(() => {
    if (!full) return;
    const onChange = () => {
      if (!document.fullscreenElement) leave();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // Before the app's own Escape, which would close what is behind the stage.
      e.stopPropagation();
      leave();
    };
    document.addEventListener('fullscreenchange', onChange);
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('fullscreenchange', onChange);
      document.removeEventListener('keydown', onKey, true);
    };
  }, [full, leave]);
  // A call that ends, or a share that stops, does not leave the window full screen.
  useEffect(
    () => () => {
      setWindow('set_fullscreen', false);
      if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    },
    [],
  );
  useEffect(
    () => () => {
      setWindow('set_always_on_top', false);
    },
    [],
  );

  return (
    <div
      ref={block}
      data-share-stage={main.rootId}
      data-share-full={full || undefined}
      className={cn('flex min-h-0 min-w-0 flex-col gap-s-2 p-s-2', full && 'fixed inset-0 z-50 bg-black')}
    >
      <div
        data-share-main={main.rootId}
        onDoubleClick={() => (full ? leave() : enter())}
        className="group relative min-h-0 flex-1 overflow-hidden rounded-lg bg-black"
      >
        <ScreenPicture key={main.rootId} sharer={main} onSize={setHeight} />
        <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-[12px] font-medium text-text-strong">
          <Monitor className="size-3.5" aria-hidden="true" />
          {main.self ? `${main.name} (you)` : main.name}
        </span>
        <div className="absolute top-2.5 left-2.5 flex items-center gap-1.5">
          {height ? <span className="inline-flex h-5 items-center rounded bg-black/55 px-1.5 text-[11px] font-semibold text-text-strong" data-share-resolution>{`${height}p`}</span> : null}
          <LiveBadge />
        </div>
        <div className="absolute top-2.5 right-2.5 flex items-center gap-1.5">
          <StageButton
            label={stripOpen ? 'Hide people' : 'Show people'}
            onClick={() => setStripOpen(!stripOpen)}
          >
            {stripOpen ? <ChevronDown className="size-4" aria-hidden="true" /> : <ChevronUp className="size-4" aria-hidden="true" />}
          </StageButton>
          <StageButton
            label={onTop ? 'Stop staying on top' : 'Stay on top'}
            pressed={onTop}
            onClick={() => {
              setWindow('set_always_on_top', !onTop);
              setOnTop(!onTop);
            }}
          >
            {onTop ? <PinOff className="size-4" aria-hidden="true" /> : <Pin className="size-4" aria-hidden="true" />}
          </StageButton>
          <StageButton label={full ? 'Exit full screen' : 'Full screen'} onClick={() => (full ? leave() : enter())}>
            {full ? <Minimize className="size-4" aria-hidden="true" /> : <Maximize className="size-4" aria-hidden="true" />}
          </StageButton>
        </div>
      </div>
      {stripOpen && blockHeight >= STRIP_MIN_STAGE ? (
        <div data-share-strip className="dissent-scroll flex h-[clamp(72px,18%,132px)] shrink-0 justify-center-safe gap-s-2 overflow-x-auto [&>*]:aspect-video [&>*]:h-full [&>*]:shrink-0">
          {others.map((sharer) => (
            <button
              key={sharer.rootId}
              type="button"
              aria-label={`Watch ${sharer.name}'s screen`}
              onClick={() => setPicked(sharer.rootId)}
              className="relative overflow-hidden rounded-lg bg-black focus-visible:outline-2 focus-visible:outline-ring"
            >
              <ScreenPicture sharer={sharer} />
              <span className="absolute top-1.5 right-1.5"><LiveBadge /></span>
              <span className="absolute bottom-1.5 left-1.5 inline-flex h-6 items-center gap-1 rounded-md bg-black/55 px-2 text-[12px] text-text-strong">
                <Monitor className="size-3.5" aria-hidden="true" />
                {sharer.self ? 'You' : sharer.name}
              </span>
            </button>
          ))}
          {people}
        </div>
      ) : null}
    </div>
  );
}
