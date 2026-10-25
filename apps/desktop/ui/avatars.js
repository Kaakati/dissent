// Profile pictures and server icons. The engine sends `avatar` on people (keyed by root_id, or peer on a conversation)
// and `icon` on servers (keyed by id). Each is a URL the webview can load, or null. Every API response passes through
// noteImages(), so a renderer looks an image up by the id it already has. Sources are content-addressed, so the same
// picture under two ids shares one decoded copy.
const images = new Map(); // id -> src
const decoded = new Map(); // src -> HTMLImageElement, kept so a re-render does not decode again

export function noteImages(value) {
  if (Array.isArray(value)) {
    for (const item of value) noteImages(item);
    return;
  }
  if (!value || typeof value !== 'object') return;
  const key = value.root_id ?? value.peer;
  if (typeof key === 'string' && 'avatar' in value) setImage(key, value.avatar);
  if (typeof value.id === 'string' && 'icon' in value) setImage(value.id, value.icon);
  for (const child of Object.values(value)) if (child && typeof child === 'object') noteImages(child);
}

function setImage(id, src) {
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

export const imageOf = (id) => (id ? images.get(id) ?? null : null);
