// Hardware video decoding. Received camera and screen frames arrive still encoded, over a binary Channel, and this window
// decodes them with WebCodecs and draws each tile to its canvas (see video-decoders.ts). Off by default until it is measured
// in release builds; the viewer can turn it on. Without WebCodecs, or without H.264 in it, the engine keeps sending JPEG previews and nothing here runs. A tile
// that fails to decode falls back to JPEG previews for the rest of the call.
import { useSyncExternalStore } from 'react';
import { api } from './api';
import { EncodedTiles, type CanvasLike, type DecoderLike } from './video-decoders';

const SETTING = 'dissent.hardwareVideo';
const H264 = 'avc1.42E01F';
const ASK_EVERY_MS = 1000;

// Tauri's Channel, as the global exposes it. The app's global type does not declare it.
type ChannelConstructor = new <T>() => { onmessage: (message: T) => void };

let listening = false;
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
    void api.setJpegTiles([...tiles.fallbacks()]).catch(() => {});
  },
});

/** The saved choice, off by default: the window decodes received video itself only when the viewer turned that on. Stored
 *  per viewer, because it only changes how this window decodes. */
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

/** Whether a peer's tiles draw canvases: frames are reaching this window encoded, and that peer has not fallen back. */
export function useEncodedVideoOn(root: string): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => listening && !tiles.fallbacks().has(root),
  );
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

/** Turns the encoded path on or off in the engine. On only when this window can decode H.264. Resolves to what is on. */
export async function applyHardwareVideo(on: boolean): Promise<boolean> {
  if (!on || !(await webcodecsSupported())) {
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
    notify();
    requestKeyframes(true);
  }
  return true;
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

