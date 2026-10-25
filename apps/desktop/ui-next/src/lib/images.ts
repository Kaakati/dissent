// Profile pictures and server icons. The engine sends `avatar` on people (keyed by root_id, or peer on a conversation)
// and `icon` on servers (keyed by id). Each is a URL the webview can load, or null. Every API response passes through
// noteImages(), so a component looks an image up by the id it already has. Ported from ui/avatars.js.

const images = new Map<string, string>(); // id -> src
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
  for (const child of Object.values(record)) if (child && typeof child === 'object') noteImages(child);
}

export function imageOf(id: string | null | undefined): string | null {
  return id ? images.get(id) ?? null : null;
}
