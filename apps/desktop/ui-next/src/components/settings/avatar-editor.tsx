// Profile pictures, server icons and server banners. A picked file opens "Edit Image", where the picture is panned and
// zoomed under a mask. Apply draws the crop onto a canvas (256 px square, or 960 x 540 for a banner) and encodes it, so
// no original bytes (EXIF included) leave the app. Ported from views/avatar.js. chooseImage() is shared with the server
// icon dialog and Server Settings.
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { Image as ImageIcon, Loader2, RotateCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { api } from '@/lib/api';
import { openDialog } from '@/lib/dialog';
import { toast } from '@/lib/toasts';
import { cn } from '@/lib/utils';

const TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
const MAX_FILE = 10 * 1024 * 1024;
const MASK = 0.8; // mask width as a share of the stage; matches the mask's inset of 10%
const MAX_ZOOM = 5; // the closest zoom, as a multiple of the zoom that just covers the mask
const PAN_STEP = 8; // px per arrow key; Shift moves four times as far
const ZOOM_STEP = 1.1;

export type CropShape = 'circle' | 'rounded' | 'banner';

// The exported size of each shape, and the most it may weigh. The engine refuses anything larger.
const OUTPUT: Record<CropShape, { w: number; h: number; maxBytes: number }> = {
  circle: { w: 256, h: 256, maxBytes: 256 * 1024 },
  rounded: { w: 256, h: 256, maxBytes: 256 * 1024 },
  banner: { w: 960, h: 540, maxBytes: 512 * 1024 },
};

// The mask in stage px: MASK of the stage wide, and as tall as the shape's aspect makes it.
const maskOf = (side: number, shape: CropShape) => {
  const w = side * MASK;
  return { w, h: (w * OUTPUT[shape].h) / OUTPUT[shape].w };
};

// What Apply hands back: the finished picture as base64, and for an animated GIF banner, its still poster frame too.
// An animated banner is not drawn on a canvas, which keeps one frame: the engine crops and loops it (see
// `animated_banner` in dissent-core), while the editor shows a spinner.
export type ApplyImage = (base64: string, animated?: { poster: string }) => unknown;

// Opens a file picker. `onApply` gets the finished picture, once the user applies a crop.
// shape: 'circle' for people, 'rounded' for server icons, 'banner' for the 16:9 server banner.
export function chooseImage({ shape, onApply }: { shape: CropShape; onApply: ApplyImage }): void {
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = TYPES.join(',');
  input.addEventListener('change', () => {
    const file = input.files?.[0];
    if (file) acceptFile(file, shape, onApply);
  });
  input.click();
}

// Whether a GIF has more than one frame: each frame of an animation has its own graphic control extension.
export function isAnimatedGif(bytes: Uint8Array): boolean {
  if (bytes[0] !== 0x47 || bytes[1] !== 0x49 || bytes[2] !== 0x46) return false;
  let controls = 0;
  for (let i = 0; i + 2 < bytes.length; i++) {
    if (bytes[i] === 0x21 && bytes[i + 1] === 0xf9 && bytes[i + 2] === 0x04 && ++controls > 1) return true;
  }
  return false;
}

function acceptFile(file: File, shape: CropShape, onApply: ApplyImage): void {
  if (!TYPES.includes(file.type)) {
    toast('Pick a PNG, JPEG, WebP or GIF image');
    return;
  }
  if (file.size > MAX_FILE) {
    toast('Pick an image up to 10 MB');
    return;
  }
  const url = URL.createObjectURL(file);
  const picture = new Image();
  picture.src = url;
  const animated = shape === 'banner' && file.type === 'image/gif'
    ? file.arrayBuffer().then((buf) => (isAnimatedGif(new Uint8Array(buf)) ? new Uint8Array(buf) : null))
    : Promise.resolve(null);
  Promise.all([picture.decode(), animated]).then(
    ([, gif]) => {
      openDialog({
        title: 'Edit Image',
        className: 'avatar-dialog',
        onClose: () => URL.revokeObjectURL(url),
        content: (close) => <CropEditor picture={picture} url={url} shape={shape} gif={gif} onApply={onApply} close={close} />,
      });
    },
    () => {
      URL.revokeObjectURL(url);
      toast('That file could not be read as an image');
    },
  );
}

interface Crop {
  rot: number; // quarter turns clockwise
  scale: number; // screen px per natural px; 0 until the stage has a size, which means "the fit"
  x: number; // picture centre from stage centre, in stage px
  y: number;
}

// The picture's width and height on screen axes, once turned.
const turnedSize = (picture: HTMLImageElement, rot: number) =>
  rot % 2 === 1 ? { w: picture.naturalHeight, h: picture.naturalWidth } : { w: picture.naturalWidth, h: picture.naturalHeight };

// The smallest scale at which the picture still covers the mask. For a square mask rotation does not change it.
const fitOf = (side: number, picture: HTMLImageElement, shape: CropShape, rot = 0): number => {
  if (!side) return 0;
  const mask = maskOf(side, shape);
  const size = turnedSize(picture, rot);
  return Math.max(mask.w / size.w, mask.h / size.h);
};

// Pan and zoom are clamped so the mask is always covered by the picture.
function clampCrop(c: Crop, side: number, picture: HTMLImageElement, shape: CropShape): Crop {
  const fit = fitOf(side, picture, shape, c.rot);
  const size = turnedSize(picture, c.rot);
  const mask = maskOf(side, shape);
  const scale = Math.min(fit * MAX_ZOOM, Math.max(fit, c.scale));
  const maxX = Math.max(0, (size.w * scale - mask.w) / 2);
  const maxY = Math.max(0, (size.h * scale - mask.h) / 2);
  return { ...c, scale, x: Math.min(maxX, Math.max(-maxX, c.x)), y: Math.min(maxY, Math.max(-maxY, c.y)) };
}

function CropEditor({
  picture,
  url,
  shape,
  gif,
  onApply,
  close,
}: {
  picture: HTMLImageElement;
  url: string;
  shape: CropShape;
  gif: Uint8Array | null; // the original bytes of an animated GIF banner, which the engine re-encodes
  onApply: ApplyImage;
  close: () => void;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef(0);
  const [side, setSide] = useState(0);
  const [crop, setCrop] = useState<Crop>({ rot: 0, scale: 0, x: 0, y: 0 });
  const [busy, setBusy] = useState(false);

  const fit = fitOf(side, picture, shape, crop.rot);
  const out = OUTPUT[shape];
  // Until the first change the crop sits at the fit scale.
  const current: Crop = { ...crop, scale: crop.scale || fit };
  const change = (fn: (c: Crop) => Crop) =>
    setCrop((c) => clampCrop(fn({ ...c, scale: c.scale || fitOf(side, picture, shape, c.rot) }), side, picture, shape));
  const zoomBy = (factor: number) => change((c) => ({ ...c, scale: c.scale * factor }));
  const rotate = () => change((c) => ({ ...c, rot: (c.rot + 1) % 4 }));
  const pan = (dx: number, dy: number) => change((c) => ({ ...c, x: c.x + dx, y: c.y + dy }));

  // Measure the stage. The stage takes focus as the dialog opens, so the keys work at once.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const measure = () => setSide(stage.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(stage);
    stage.focus();
    return () => observer.disconnect();
  }, []);

  // The wheel zooms. It is attached natively so the page does not scroll and the listener can cancel the default.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      setCrop((c) =>
        clampCrop({ ...c, scale: (c.scale || fitOf(side, picture, shape, c.rot)) * Math.exp(-e.deltaY * 0.0015) }, side, picture, shape),
      );
    };
    stage.addEventListener('wheel', onWheel, { passive: false });
    return () => stage.removeEventListener('wheel', onWheel);
  }, [side, picture, shape]);

  // ---- pointer: drag to pan, two fingers to zoom ----
  const gap = () => {
    const [a, b] = [...pointers.current.values()];
    return Math.hypot(a.x - b.x, a.y - b.y);
  };
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    pinch.current = pointers.current.size === 2 ? gap() : 0;
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const prev = pointers.current.get(e.pointerId);
    if (!prev) return;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.current.size === 1) {
      pan(e.clientX - prev.x, e.clientY - prev.y);
    } else if (pointers.current.size === 2) {
      const next = gap();
      if (pinch.current) zoomBy(next / pinch.current);
      pinch.current = next;
    }
  };
  const release = (e: PointerEvent<HTMLDivElement>) => {
    pointers.current.delete(e.pointerId);
    pinch.current = 0;
  };

  // ---- keys: arrows pan (the slider keeps its own), + and - zoom, R rotates, Enter applies ----
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (e.key === 'Enter' && target.tagName !== 'BUTTON') {
      e.preventDefault();
      void apply();
      return;
    }
    const step = e.shiftKey ? PAN_STEP * 4 : PAN_STEP;
    const keys: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
    if (keys[e.key] && target.getAttribute('type') !== 'range') {
      e.preventDefault();
      pan(...keys[e.key]);
    } else if (e.key === '+' || e.key === '=') {
      zoomBy(ZOOM_STEP);
    } else if (e.key === '-' || e.key === '_') {
      zoomBy(1 / ZOOM_STEP);
    } else if (e.key === 'r' || e.key === 'R') {
      rotate();
    }
  };

  // ---- export: draw the crop at the shape's size, then encode WebP at 0.85 with PNG as the fallback ----
  const encode = (type: string, quality?: number) =>
    new Promise<Blob | null>((resolve) => {
      const canvas = document.createElement('canvas');
      canvas.width = out.w;
      canvas.height = out.h;
      const ctx = canvas.getContext('2d');
      if (!ctx) {
        resolve(null);
        return;
      }
      const k = out.w / maskOf(side, shape).w;
      ctx.imageSmoothingQuality = 'high';
      // The picture's centre sits (x, y) from the mask centre, which is the canvas centre.
      ctx.translate(out.w / 2 + current.x * k, out.h / 2 + current.y * k);
      ctx.rotate((current.rot * Math.PI) / 2);
      ctx.scale(current.scale * k, current.scale * k);
      ctx.drawImage(picture, -picture.naturalWidth / 2, -picture.naturalHeight / 2);
      canvas.toBlob(resolve, type, quality);
    });
  const exportCrop = async (): Promise<Blob | null> => {
    const webp = await encode('image/webp', 0.85);
    return webp?.type === 'image/webp' ? webp : encode('image/png');
  };
  // The crop in source pixels of the turned picture: where the mask sits over it, at its natural size.
  const cropRect = () => {
    const size = turnedSize(picture, current.rot);
    const mask = maskOf(side, shape);
    const w = Math.round(mask.w / current.scale);
    const h = Math.round(mask.h / current.scale);
    const x = Math.round(size.w / 2 - current.x / current.scale - w / 2);
    const y = Math.round(size.h / 2 - current.y / current.scale - h / 2);
    return { x: Math.max(0, x), y: Math.max(0, y), w: Math.max(1, w), h: Math.max(1, h), rot: current.rot };
  };
  const applyAnimated = async (bytes: Uint8Array) => {
    try {
      const prepared = await api.prepareAnimatedBanner(base64(bytes.slice().buffer), cropRect());
      close();
      await onApply(prepared.gif, { poster: prepared.poster });
    } catch (err) {
      toast(err);
      setBusy(false);
    }
  };
  const apply = async () => {
    if (busy) return;
    setBusy(true);
    if (gif) {
      await applyAnimated(gif);
      return;
    }
    const blob = await exportCrop();
    if (!blob) {
      setBusy(false);
      return;
    }
    if (blob.size > out.maxBytes) {
      toast(`This crop is over ${out.maxBytes / 1024} KB after compression. Try a simpler picture or a tighter crop.`);
      setBusy(false);
      return;
    }
    close();
    await onApply(base64(await blob.arrayBuffer()));
  };

  const reset = () => setCrop(clampCrop({ rot: 0, x: 0, y: 0, scale: fitOf(side, picture, shape) }, side, picture, shape));
  const zoomValue = fit ? Math.round((Math.log(current.scale / fit) / Math.log(MAX_ZOOM)) * 1000) : 0;

  return (
    <div className="flex flex-col gap-s-4" onKeyDown={onKey}>
      <div
        ref={stageRef}
        data-crop-stage
        tabIndex={0}
        role="application"
        aria-label="Crop area. Arrow keys move the picture, plus and minus zoom, R rotates."
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={release}
        onPointerCancel={release}
        className="relative mx-auto aspect-square w-full max-w-[400px] touch-none cursor-grab overflow-hidden rounded-lg bg-ink select-none active:cursor-grabbing focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
      >
        <img
          data-crop-image
          src={url}
          alt=""
          draggable={false}
          className="pointer-events-none absolute max-w-none origin-center"
          style={{
            width: `${picture.naturalWidth}px`,
            height: `${picture.naturalHeight}px`,
            left: `${(side - picture.naturalWidth) / 2}px`,
            top: `${(side - picture.naturalHeight) / 2}px`,
            transform: `translate(${current.x}px, ${current.y}px) rotate(${current.rot * 90}deg) scale(${current.scale})`,
          }}
        />
        <svg className="pointer-events-none absolute inset-0 size-full fill-black/60" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <path d={maskPath(shape)} fillRule="evenodd" />
        </svg>
        <div
          data-crop-mask
          className={cn(
            'pointer-events-none absolute inset-x-[10%] border-3 border-white',
            shape === 'circle' && 'rounded-full',
            shape === 'rounded' && 'rounded-[33%]',
            shape === 'banner' && 'rounded-lg',
          )}
          style={{ top: `${maskTop(shape)}%`, bottom: `${maskTop(shape)}%` }}
        />
      </div>

      <div className="flex items-center gap-s-3">
        <ImageIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <input
          type="range"
          min={0}
          max={1000}
          value={zoomValue}
          aria-label="Zoom"
          onChange={(e) => change((c) => ({ ...c, scale: fit * Math.pow(MAX_ZOOM, Number(e.target.value) / 1000) }))}
          className="flex-1 accent-accent"
        />
        <ImageIcon className="size-[22px] shrink-0 text-muted-foreground" aria-hidden="true" />
        <Button variant="ghost" size="icon" aria-label="Rotate 90°" title="Rotate 90°" onClick={rotate}>
          <RotateCw />
        </Button>
      </div>

      <div className="-mx-s-5 -mb-s-5 mt-s-1 flex items-center gap-s-2 bg-sidebar px-s-5 py-s-4">
        <Button variant="ghost" className="mr-auto px-s-2 font-medium text-accent-text" onClick={reset}>
          Reset
        </Button>
        <Button variant="outline" onClick={close}>
          Cancel
        </Button>
        <Button disabled={busy} aria-busy={busy || undefined} onClick={() => void apply()}>
          {busy && gif ? (
            <>
              <Loader2 className="animate-spin" aria-hidden="true" />
              Encoding…
            </>
          ) : (
            'Apply'
          )}
        </Button>
      </div>
    </div>
  );
}

// How far the mask sits from the top (and bottom) of the stage, in percent. 10 for a square mask; a banner's mask is
// as wide but shorter, so it sits lower.
function maskTop(shape: CropShape): number {
  return 50 - (MASK * 50 * OUTPUT[shape].h) / OUTPUT[shape].w;
}

// The dim layer with the mask cut out (even-odd), in a 100 x 100 box that matches the stage. The mask is 80% of the
// stage wide, from 10 to 90; a server icon's corners have a radius of 33% of it, a banner's a small one.
function maskPath(shape: CropShape): string {
  const top = maskTop(shape);
  const bottom = 100 - top;
  const rect = (r: number) =>
    `M${10 + r} ${top}H${90 - r}A${r} ${r} 0 0 1 90 ${top + r}V${bottom - r}A${r} ${r} 0 0 1 ${90 - r} ${bottom}H${10 + r}A${r} ${r} 0 0 1 10 ${bottom - r}V${top + r}A${r} ${r} 0 0 1 ${10 + r} ${top}Z`;
  const hole = shape === 'circle' ? 'M10 50a40 40 0 1 0 80 0a40 40 0 1 0-80 0Z' : rect(shape === 'rounded' ? 80 * 0.33 : 2);
  return `M0 0H100V100H0Z ${hole}`;
}

function base64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}
