// The banner behind a profile: the hue of the person's id, as the current UI's .banner gradient.
import { hueOf } from '@/lib/identity';

export function bannerBackground(seed: string): string {
  const hue = hueOf(seed);
  return `linear-gradient(135deg, oklch(45% 0.11 ${hue}), oklch(32% 0.08 ${hue}))`;
}
