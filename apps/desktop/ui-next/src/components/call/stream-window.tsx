// The pop-out: one shared screen in a window of its own, opened from the call's stage. It decodes the stream itself, and the call
// goes on in the main window, which closes this one when the share or the call ends. It loads nothing but this, so it needs no
// profile, store or engine state of its own.
import { useEffect, useRef, useState } from 'react';
import { Maximize, Minimize, Monitor, Pin, PinOff } from 'lucide-react';
import { api } from '@/lib/api';
import { EncodedTiles, type CanvasLike, type DecoderLike } from '@/lib/video-decoders';

type ChannelConstructor = new <T>() => { onmessage: (message: T) => void };

function setWindow(command: 'set_fullscreen' | 'set_always_on_top', value: boolean) {
  void window.__TAURI__?.core.invoke(`plugin:window|${command}`, { label: 'stream', value })?.catch(() => {});
}

export function StreamWindow({ rootId, name }: { rootId: string; name: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [full, setFull] = useState(false);
  const [onTop, setOnTop] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const tiles = new EncodedTiles({
      makeDecoder: (init) => new VideoDecoder(init) as unknown as DecoderLike,
      makeChunk: (init) => new EncodedVideoChunk(init),
      now: () => Date.now(),
      requestKeyframes: () => void api.requestVideoKeyframes().catch(() => {}),
      switchedToJpeg: () => setFailed(true),
    });
    const key = `${rootId}:screen`;
    tiles.register(key, canvas.current as unknown as CanvasLike);
    const Channel = (window.__TAURI__ as unknown as { core: { Channel?: ChannelConstructor } } | undefined)?.core.Channel;
    if (Channel) {
      const channel = new Channel<ArrayBuffer>();
      channel.onmessage = (message) => tiles.receive(message);
      void api.subscribeStreamWindow(rootId, channel).then(() => api.requestVideoKeyframes()).catch(() => setFailed(true));
    }
    return () => tiles.register(key, null);
  }, [rootId]);

  // The window's width picks the layer a forwarder sends.
  useEffect(() => {
    const report = () => void api.setVideoTile(rootId, true, Math.round(window.innerWidth * window.devicePixelRatio)).catch(() => {});
    report();
    window.addEventListener('resize', report);
    return () => window.removeEventListener('resize', report);
  }, [rootId]);

  const toggleFull = () => {
    setWindow('set_fullscreen', !full);
    setFull(!full);
  };
  useEffect(() => {
    if (!full) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      setWindow('set_fullscreen', false);
      setFull(false);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [full]);

  const button = 'grid size-8 place-items-center rounded-md bg-black/55 text-text-strong hover:bg-black/75 focus-visible:outline-2 focus-visible:outline-ring';
  return (
    <div data-stream-window={rootId} onDoubleClick={toggleFull} className="fixed inset-0 bg-black">
      <canvas ref={canvas} aria-label={`${name}'s screen`} className="absolute inset-0 size-full object-contain" />
      {failed ? <p className="absolute inset-0 m-auto h-fit text-center text-sm text-text">This stream can’t be shown here. Close this window to watch it in the call.</p> : null}
      <span className="absolute bottom-2.5 left-2.5 inline-flex h-7 items-center gap-1.5 rounded-md bg-black/55 px-2.5 text-[12px] font-medium text-text-strong">
        <Monitor className="size-3.5" aria-hidden="true" />
        {name}
      </span>
      <div className="absolute top-2.5 right-2.5 flex gap-1.5">
        <button
          type="button"
          aria-label={onTop ? 'Stop staying on top' : 'Stay on top'}
          aria-pressed={onTop}
          className={button}
          onClick={() => {
            setWindow('set_always_on_top', !onTop);
            setOnTop(!onTop);
          }}
        >
          {onTop ? <PinOff className="size-4" aria-hidden="true" /> : <Pin className="size-4" aria-hidden="true" />}
        </button>
        <button type="button" aria-label={full ? 'Exit full screen' : 'Full screen'} className={button} onClick={toggleFull}>
          {full ? <Minimize className="size-4" aria-hidden="true" /> : <Maximize className="size-4" aria-hidden="true" />}
        </button>
      </div>
    </div>
  );
}
