// Sealed pictures: the paths the engine writes, as URLs through the app's `dimg` scheme. No imports, so the unit
// tests load it as it is.

// A picture the engine keeps sealed at rest (an avatar, an icon, a banner, a thumbnail, a link preview's picture): its
// path through the app's `dimg` scheme, which opens it. The asset protocol would only hand the window ciphertext. A
// value that is already a URL is returned as it is.
export function pictureSrc(path: string | null | undefined): string | null {
  if (!path) return null;
  if (!isLocalPath(path)) return path;
  const src = window.__TAURI__?.core.convertFileSrc?.(path, 'dimg');
  return typeof src === 'string' ? src : null;
}

// An absolute file path as the engine writes one: `C:\…` or `\\server\…` on Windows, `/…` elsewhere.
export function isLocalPath(value: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\\\') || (value.startsWith('/') && !value.startsWith('//'));
}

const PICTURE_KEYS = new Set(['avatar', 'icon', 'icon_poster', 'banner', 'banner_poster']);

// Turns the local picture paths in a response into `dimg` URLs, under the keys the backend's `for_ui` converts. Most
// commands convert already; this covers the ones that return an engine view as it is (adding a contact, creating or
// joining a server), so every `avatar`, `icon` and `banner` a component reads is a URL. Without the Tauri globals (the
// mock) a path is left as it is.
export function pictureUrls(value: unknown): void {
  if (Array.isArray(value)) {
    value.forEach(pictureUrls);
    return;
  }
  if (!value || typeof value !== 'object') return;
  const record = value as Record<string, unknown>;
  for (const [key, field] of Object.entries(record)) {
    if (PICTURE_KEYS.has(key) && typeof field === 'string' && isLocalPath(field)) record[key] = pictureSrc(field) ?? field;
    else if (field && typeof field === 'object') pictureUrls(field);
  }
}
