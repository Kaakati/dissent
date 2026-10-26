// Server banners: the ten preset gradients, by the id the engine stores (0 to 9), and the background a banner shows.
// A preset is drawn here, so it costs the server a byte, not a picture. An uploaded banner is a 16:9 picture URL.

export const BANNER_PRESETS: { name: string; background: string }[] = [
  { name: 'White', background: 'linear-gradient(135deg, #f4f5f8 0%, #c8ccd8 100%)' },
  { name: 'Pink', background: 'linear-gradient(135deg, #f7a8d0 0%, #d6489a 100%)' },
  { name: 'Red', background: 'linear-gradient(135deg, #f58a7c 0%, #c62a3a 100%)' },
  { name: 'Orange', background: 'linear-gradient(135deg, #fbbf6a 0%, #e0611d 100%)' },
  { name: 'Yellow', background: 'linear-gradient(135deg, #fbe58a 0%, #e2b21c 100%)' },
  { name: 'Purple', background: 'linear-gradient(135deg, #c3a5f7 0%, #6d3fd1 100%)' },
  { name: 'Blue', background: 'linear-gradient(135deg, #8fc4fb 0%, #2f62d8 100%)' },
  { name: 'Teal', background: 'linear-gradient(135deg, #8ee8dc 0%, #14958c 100%)' },
  { name: 'Green', background: 'linear-gradient(135deg, #a6e7a0 0%, #2b9a47 100%)' },
  { name: 'Dark', background: 'linear-gradient(135deg, #4a4d57 0%, #17181c 100%)' },
];

// What a banner looks like: an uploaded picture, a preset, or neither. An animated GIF banner has a still `poster`
// beside it; an unsaved one carries both as base64 (`bytes`, `posterBytes`).
export type BannerDraft =
  | { kind: 'image'; url: string; bytes?: string; poster?: string; posterBytes?: string }
  | { kind: 'preset'; id: number }
  | null;

export function bannerOf(image: string | null | undefined, preset: number | null | undefined, poster?: string | null): BannerDraft {
  if (image) return poster ? { kind: 'image', url: image, poster } : { kind: 'image', url: image };
  if (typeof preset === 'number' && BANNER_PRESETS[preset]) return { kind: 'preset', id: preset };
  return null;
}

// Animated banners hold still under reduced motion, and while the window is in the background, where nobody watches
// them. The page follows the window's focus; the reduced-motion setting is read each time.
let windowFocused = true;
const motionListeners = new Set<() => void>();
if (typeof window !== 'undefined') {
  const set = (focused: boolean) => {
    windowFocused = focused;
    motionListeners.forEach((l) => l());
  };
  window.addEventListener('blur', () => set(false));
  window.addEventListener('focus', () => set(true));
  window.matchMedia?.('(prefers-reduced-motion: reduce)').addEventListener?.('change', () => motionListeners.forEach((l) => l()));
}
export function bannersStill(): boolean {
  return !windowFocused || (typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches);
}
// Subscribes a component to the still/animated switch (a useSyncExternalStore source).
export const subscribeBannerMotion = (listener: () => void) => {
  motionListeners.add(listener);
  return () => motionListeners.delete(listener);
};

// The URL a banner shows now: the poster frame of an animated banner when it holds still, otherwise the picture.
export function bannerUrl(banner: BannerDraft): string | null {
  if (!banner || banner.kind !== 'image') return null;
  return banner.poster && bannersStill() ? banner.poster : banner.url;
}

// The CSS background of a banner, or null when there is none.
export function bannerBackground(banner: BannerDraft): string | null {
  if (!banner) return null;
  if (banner.kind === 'preset') return BANNER_PRESETS[banner.id]?.background ?? null;
  return `center / cover no-repeat url("${bannerUrl(banner)}")`;
}

export const sameBanner = (a: BannerDraft, b: BannerDraft): boolean =>
  a === b || (!!a && !!b && a.kind === b.kind && (a.kind === 'preset' ? a.id === (b as { id: number }).id : a.url === (b as { url: string }).url));

// A local URL for a cropped picture that is not saved yet. The app's CSP allows blob: images but not data: ones, so a
// draft banner is shown from a blob. The previous draft's URL is released when a new one is made.
const draftUrls = new Map<string, string>();
export function draftUrlOf(base64: string, slot: 'picture' | 'poster' = 'picture'): string {
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  const type = base64.startsWith('iVBOR') ? 'image/png' : base64.startsWith('R0lG') ? 'image/gif' : base64.startsWith('/9j/') ? 'image/jpeg' : 'image/webp';
  const old = draftUrls.get(slot);
  if (old) URL.revokeObjectURL(old);
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  draftUrls.set(slot, url);
  return url;
}
