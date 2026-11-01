// The banner behind a profile: the person's own banner (an uploaded picture or a preset) when they set one, then their
// profile's primary colour, otherwise the hue of their id, as the current UI's .banner gradient.
import { bannerBackground as serverBanner, bannerOf } from '@/lib/banners';
import { hueOf } from '@/lib/identity';
import { cssRgb, validTheme } from '@/lib/profile-theme';

export function bannerBackground(
  seed: string,
  view?: { banner?: string | null; banner_preset?: number | null; banner_poster?: string | null; theme?: unknown } | null,
): string {
  const own = view ? serverBanner(bannerOf(view.banner, view.banner_preset, view.banner_poster)) : null;
  if (own) return own;
  const theme = validTheme(view?.theme);
  if (theme) return cssRgb(theme.primary);
  const hue = hueOf(seed);
  return `linear-gradient(135deg, oklch(45% 0.11 ${hue}), oklch(32% 0.08 ${hue}))`;
}
