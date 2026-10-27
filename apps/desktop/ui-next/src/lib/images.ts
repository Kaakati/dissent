// Profile pictures and server icons. The engine sends `avatar` on people (keyed by root_id, or peer on a conversation)
// and `icon` on servers (keyed by id). Each is a URL the webview can load, or null. Every API response passes through
// noteImages(), so a component looks an image up by the id it already has. Ported from the old UI's avatars.js.

import { bannersStill } from './banners';

const images = new Map<string, string>(); // id -> src
const posters = new Map<string, string>(); // server id -> the still frame of its animated icon
const decoded = new Map<string, HTMLImageElement>(); // src -> image, kept so a re-render does not decode again

function setImage(id: string, src: unknown): void {
  const old = images.get(id);
  if (typeof src !== 'string' || !src) {
    images.delete(id);
  } else {
    images.set(id, src);
    if (!decoded.has(src)) {
      const img = new Image();
      img.src = src;
      decoded.set(src, img);
    }
  }
  // Drop the decoded copy once no id uses the old picture any more.
  if (old && old !== src && ![...images.values()].includes(old)) decoded.delete(old);
}

export function noteImages(value: unknown): void {
  if (Array.isArray(value)) {
    for (const item of value) noteImages(item);
    return;
  }
  if (!value || typeof value !== 'object') return;
  const record = value as Record<string, unknown>;
  const key = record.root_id ?? record.peer;
  if (typeof key === 'string' && 'avatar' in record) setImage(key, record.avatar);
  if (typeof record.id === 'string' && 'icon' in record) setImage(record.id, record.icon);
  if (typeof record.id === 'string' && 'icon_poster' in record) setPoster(record.id, record.icon_poster);
  for (const child of Object.values(record)) if (child && typeof child === 'object') noteImages(child);
}

function setPoster(id: string, src: unknown): void {
  if (typeof src === 'string' && src) posters.set(id, src);
  else posters.delete(id);
}

export function imageOf(id: string | null | undefined): string | null {
  return id ? images.get(id) ?? null : null;
}

// A server icon's URL now: an animated icon's still frame while motion is held (reduced motion, or the window in the
// background), otherwise the icon itself. The caller subscribes to subscribeBannerMotion so it redraws when that changes.
export function iconSrc(id: string | null | undefined): string | null {
  const poster = id ? posters.get(id) : undefined;
  return poster && bannersStill() ? poster : imageOf(id);
}
