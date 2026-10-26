// The banner behind a profile: the person's own banner (an uploaded picture or a preset) when they set one, otherwise
// the hue of their id, as the current UI's .banner gradient.
import { bannerBackground as serverBanner, bannerOf } from '@/lib/banners';
import { hueOf } from '@/lib/identity';

export function bannerBackground(
  seed: string,
  view?: { banner?: string | null; banner_preset?: number | null; banner_poster?: string | null } | null,
): string {
  const own = view ? serverBanner(bannerOf(view.banner, view.banner_preset, view.banner_poster)) : null;
  if (own) return own;
  const hue = hueOf(seed);
  return `linear-gradient(135deg, oklch(45% 0.11 ${hue}), oklch(32% 0.08 ${hue}))`;
}
