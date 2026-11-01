// Video decoded in this window with WebCodecs. Received frames arrive still encoded, over a binary Channel, and each tile draws to
// its canvas (see video-decoders.ts). A shared screen always comes this way when the window can decode it: JPEG previews cost
// about twice the CPU at native size and can freeze the window on AV1 (docs/design/04-calls.md, "Screen share"). Cameras come this
// way only if the viewer turned that on. Without WebCodecs, or without H.264 in it, the engine sends JPEG previews and nothing here
// runs. A tile that fails to decode falls back to JPEG previews for the rest of the call.
import { useSyncExternalStore } from 'react';
import { api } from './api';
import { EncodedTiles, type CanvasLike, type DecoderLike } from './video-decoders';

const SETTING = 'dissent.hardwareVideo';
const H264 = 'avc1.42E033';
const ASK_EVERY_MS = 1000;

// Tauri's Channel, as the global exposes it. The app's global type does not declare it.
type ChannelConstructor = new <T>() => { onmessage: (message: T) => void };

let listening = false;
let cameras = false;
let lastAsk = 0;
const listeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) listener();
}

/** Asks the peers for a keyframe. Throttled, unless `now` is set. */
function requestKeyframes(now = false) {
  const at = Date.now();
  if (!now && at - lastAsk < ASK_EVERY_MS) return;
  lastAsk = at;
  void api.requestVideoKeyframes().catch(() => {});
}

const tiles = new EncodedTiles({
  makeDecoder: (init) => new VideoDecoder(init) as unknown as DecoderLike,
  makeChunk: (init) => new EncodedVideoChunk(init),
  now: () => Date.now(),
  requestKeyframes: () => requestKeyframes(),
  switchedToJpeg: (root, reason) => {
    console.warn(`Dissent: video from ${root} uses JPEG previews for the rest of this call (${reason}).`);
    notify();
    // The engine's decoder starts at a keyframe, and H.264 sends one only when asked.
    void api.setJpegTiles([...tiles.fallbacks()]).then(() => requestKeyframes(true)).catch(() => {});
  },
});

/** The saved choice, off by default: the window decodes cameras itself only when the viewer turned that on. Stored per viewer,
 *  because it only changes how this window decodes. */
export function hardwareVideoWanted(): boolean {
  try {
    return localStorage.getItem(SETTING) === '1';
  } catch {
    return false;
  }
}

export function saveHardwareVideoWanted(on: boolean) {
  try {
    localStorage.setItem(SETTING, on ? '1' : '0');
  } catch {
    // A window that cannot store the choice keeps it for this session only.
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Whether a peer's camera tile draws a canvas: this window decodes cameras, and that peer has not fallen back. */
export function useEncodedVideoOn(root: string): boolean {
  return useSyncExternalStore(subscribe, () => listening && cameras && !tiles.fallbacks().has(root));
}

/** Whether a peer's shared screen draws a canvas: this window decodes, and that peer has not fallen back. */
export function useEncodedScreenOn(root: string): boolean {
  return useSyncExternalStore(subscribe, () => listening && !tiles.fallbacks().has(root));
}

/** Whether this window can decode H.264 itself, which needs the desktop app and WebCodecs. A window without hardware
 *  decoding still decodes in software, so the hardware preference is not required. */
export async function webcodecsSupported(): Promise<boolean> {
  if (typeof VideoDecoder === 'undefined' || typeof EncodedVideoChunk === 'undefined' || !window.__TAURI__) return false;
  try {
    const { supported } = await VideoDecoder.isConfigSupported({ codec: H264 });
    return supported === true;
  } catch {
    return false;
  }
}

/** Starts the encoded path, which shared screens always use when this window can decode H.264, and sets whether cameras use it
 *  too. Resolves to whether cameras do. */
export async function applyHardwareVideo(camerasOn: boolean): Promise<boolean> {
  if (!(await webcodecsSupported())) {
    await stopListening();
    return false;
  }
  if (!listening) {
    const Channel = (window.__TAURI__ as unknown as { core: { Channel?: ChannelConstructor } } | undefined)?.core.Channel;
    if (!Channel) return false;
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (message: ArrayBuffer) => onEncodedFrame(message);
    try {
      await api.subscribeEncodedVideo(channel);
    } catch {
      return false;
    }
    listening = true;
  }
  await api.setEncodedCameras(camerasOn).catch(() => {});
  cameras = camerasOn;
  notify();
  requestKeyframes(true);
  return camerasOn;
}

/** Starts a new call with every tile on the encoded path again. */
export function resetForCall() {
  if (tiles.fallbacks().size === 0) return;
  tiles.clearFallbacks();
  void api.setJpegTiles([]).catch(() => {});
  notify();
}

async function stopListening() {
  const was = listening;
  listening = false;
  tiles.closeAll();
  if (was) notify();
  // Always clear the engine's channel. A window that reloaded has no channel of its own, but the engine may still hold the
  // last one, and its frames would go nowhere.
  await api.unsubscribeEncodedVideo().catch(() => {});
}

/** Points a tile's canvas at this window's element, or forgets it when the tile goes (`el` is null). */
export function registerCanvas(key: string, el: HTMLCanvasElement | null) {
  tiles.register(key, (el as unknown as CanvasLike | null) ?? null);
}

function onEncodedFrame(message: ArrayBuffer) {
  tiles.receive(message);
}

