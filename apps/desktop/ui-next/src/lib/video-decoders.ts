// The window's decoders for received frames, one per camera or screen tile. Pure: the browser's VideoDecoder and canvases are
// passed in, so a test can drive it with mock decoders and count the frames that are still open.
//
// A tile falls back to JPEG previews for the rest of the call when its decoder fails more than ERROR_LIMIT times in
// ERROR_WINDOW_MS, or when no frame is drawn FIRST_FRAME_MS after a keyframe. The fallback is reported once per tile.
// Every frame the decoder outputs is closed once it has been drawn, including when drawing throws and when the tile is gone.
// Plain TypeScript only (no enums, no parameter properties), so Node can run it directly.

export const ERROR_LIMIT = 3;
export const ERROR_WINDOW_MS = 10_000;
export const FIRST_FRAME_MS = 3_000;

export interface FrameLike {
  displayWidth: number;
  displayHeight: number;
  close(): void;
}

export interface CanvasLike {
  width: number;
  height: number;
  hidden: boolean;
  getContext(kind: '2d'): { drawImage(source: unknown, x: number, y: number): void } | null;
}

export interface DecoderLike {
  readonly state: string;
  configure(config: { codec: string; optimizeForLatency?: boolean }): void;
  decode(chunk: unknown): void;
  close(): void;
}

export interface Deps {
  makeDecoder(init: { output: (frame: FrameLike) => void; error: (error: unknown) => void }): DecoderLike;
  makeChunk(init: { type: 'key' | 'delta'; timestamp: number; data: Uint8Array }): unknown;
  now(): number;
  requestKeyframes(): void;
  /** Called once per root when its tiles switch to JPEG previews. */
  switchedToJpeg(root: string, reason: string): void;
}

export interface Packet {
  keyframe: boolean;
  screen: boolean;
  av1: boolean;
  frameId: number;
  root: string;
  data: Uint8Array;
}

/** Reads one binary message from the engine: version 1, flags (1 keyframe, 2 screen, 4 AV1), frame id, root id, bitstream. */
export function parsePacket(buffer: ArrayBuffer): Packet | null {
  const view = new DataView(buffer);
  if (view.byteLength < 8 || view.getUint8(0) !== 1) return null;
  const flags = view.getUint8(1);
  const rootLength = view.getUint16(6, true);
  if (view.byteLength < 8 + rootLength) return null;
  return {
    keyframe: (flags & 1) !== 0,
    screen: (flags & 2) !== 0,
    av1: (flags & 4) !== 0,
    frameId: view.getUint32(2, true),
    root: new TextDecoder().decode(new Uint8Array(buffer, 8, rootLength)),
    data: new Uint8Array(buffer, 8 + rootLength),
  };
}

interface Tile {
  root: string;
  decoder: DecoderLike | undefined;
  canvas: CanvasLike | undefined;
  /** When the last keyframe arrived. Cleared once a frame has been drawn after it. */
  keyAt: number | undefined;
  drawn: boolean;
  errors: number[];
}

export class EncodedTiles {
  private readonly deps: Deps;
  private readonly tiles = new Map<string, Tile>();
  private readonly fallen = new Set<string>();

  constructor(deps: Deps) {
    this.deps = deps;
  }

  /** Points a tile's canvas at the window's element. `null` means the tile is gone: its decoder is closed. */
  register(key: string, canvas: CanvasLike | null) {
    const tile = this.tileFor(key);
    tile.canvas = canvas ?? undefined;
    if (!canvas) this.closeDecoder(tile);
  }

  /** Handles one message from the engine. */
  receive(buffer: ArrayBuffer) {
    const packet = parsePacket(buffer);
    if (!packet || this.fallen.has(packet.root)) return;
    const key = packet.screen ? `${packet.root}:screen` : packet.root;
    const tile = this.tileFor(key);
    const now = this.deps.now();

    if (tile.keyAt !== undefined && !tile.drawn && now - tile.keyAt > FIRST_FRAME_MS) {
      this.fallBack(packet.root, 'no frame was drawn within 3 s of a keyframe');
      return;
    }
    if (packet.keyframe) {
      this.startDecoder(tile, packet.av1 ? 'av01.0.08M.08' : 'avc1.42E01F');
      tile.keyAt = now;
      tile.drawn = false;
    } else if (!tile.decoder || tile.decoder.state !== 'configured') {
      this.deps.requestKeyframes();
      return;
    }
    try {
      tile.decoder?.decode(this.deps.makeChunk({ type: packet.keyframe ? 'key' : 'delta', timestamp: packet.frameId * 1000, data: packet.data }));
    } catch {
      this.failed(tile);
    }
  }

  /** Closes every decoder. Used when the encoded path stops. */
  closeAll() {
    for (const tile of this.tiles.values()) this.closeDecoder(tile);
  }

  /** The roots whose tiles have switched to JPEG previews. */
  fallbacks(): ReadonlySet<string> {
    return this.fallen;
  }

  /** Forgets the fallbacks, when a new call starts. */
  clearFallbacks() {
    this.fallen.clear();
  }

  private tileFor(key: string): Tile {
    let tile = this.tiles.get(key);
    if (!tile) {
      tile = { root: key.split(':')[0], decoder: undefined, canvas: undefined, keyAt: undefined, drawn: false, errors: [] };
      this.tiles.set(key, tile);
    }
    return tile;
  }

  private startDecoder(tile: Tile, codec: string) {
    this.closeDecoder(tile);
    const decoder: DecoderLike = this.deps.makeDecoder({
      output: (frame) => this.draw(tile, frame),
      error: () => {
        if (decoder && tile.decoder === decoder) this.failed(tile);
      },
    });
    try {
      decoder.configure({ codec, optimizeForLatency: true });
      tile.decoder = decoder;
    } catch {
      decoder.close();
      this.failed(tile);
    }
  }

  private draw(tile: Tile, frame: FrameLike) {
    try {
      const canvas = tile.canvas;
      if (canvas) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
        canvas.getContext('2d')?.drawImage(frame, 0, 0);
        canvas.hidden = false;
      }
      tile.drawn = true;
    } finally {
      frame.close();
    }
  }

  private failed(tile: Tile) {
    tile.decoder = undefined;
    const now = this.deps.now();
    tile.errors = tile.errors.filter((at) => now - at <= ERROR_WINDOW_MS);
    tile.errors.push(now);
    if (tile.errors.length > ERROR_LIMIT) {
      this.fallBack(tile.root, `the decoder failed ${tile.errors.length} times in 10 s`);
    } else {
      this.deps.requestKeyframes();
    }
  }

  private fallBack(root: string, reason: string) {
    if (this.fallen.has(root)) return;
    this.fallen.add(root);
    for (const tile of this.tiles.values()) {
      if (tile.root === root) this.closeDecoder(tile);
    }
    this.deps.switchedToJpeg(root, reason);
  }

  private closeDecoder(tile: Tile) {
    const decoder = tile.decoder;
    tile.decoder = undefined;
    tile.keyAt = undefined;
    if (decoder && decoder.state !== 'closed') decoder.close();
  }
}
