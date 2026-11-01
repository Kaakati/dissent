// Profile colours: a person's primary and accent, drawn as a vertical gradient behind their card with translucent panels
// on it. The colours come from other people, so only validTheme's numbers ever reach the CSS, and each gradient stop is
// darkened until the card's text passes WCAG AA on it, whatever was picked. The tokens the card's text uses inside the
// theme are in index.css, under [data-profile-theme].
import type { CSSProperties } from 'react';

export type Rgb = [number, number, number];
export interface ProfileTheme {
  primary: Rgb;
  accent: Rgb;
}

// The lightest text on a themed card, the same values as --profile-text and --profile-muted in index.css (a unit test
// compares them). Passing with the muted one passes with all the lighter ones.
export const THEME_TEXT = '#ffffff';
export const THEME_MUTED = '#e4e6ea';
// AA is 4.5. A browser rounds each colour of a gradient, so the stops keep a little in hand.
const AA_TEXT = 4.6;

const isByte = (n: unknown): n is number => Number.isInteger(n) && (n as number) >= 0 && (n as number) <= 255;
const isRgb = (c: unknown): c is Rgb => Array.isArray(c) && c.length === 3 && c.every(isByte);

// The theme of a view, or null for anything that is not exactly two RGB byte triples. Null means today's look.
export function validTheme(raw: unknown): ProfileTheme | null {
  if (!raw || typeof raw !== 'object') return null;
  const { primary, accent } = raw as { primary?: unknown; accent?: unknown };
  return isRgb(primary) && isRgb(accent) ? { primary: [...primary], accent: [...accent] } : null;
}

export const cssRgb = ([r, g, b]: Rgb): string => `rgb(${r} ${g} ${b})`;

const HEX = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i;
export const toHex = (c: Rgb): string => `#${c.map((n) => n.toString(16).padStart(2, '0')).join('')}`;
export function fromHex(hex: string): Rgb | null {
  const m = HEX.exec(hex);
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : null;
}

// WCAG relative luminance and contrast.
export function luminance(c: Rgb): number {
  const [r, g, b] = c.map((n) => {
    const s = n / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
export function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const scale = (c: Rgb, f: number): Rgb => [Math.floor(c[0] * f), Math.floor(c[1] * f), Math.floor(c[2] * f)];
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [0, 1, 2].map((i) => Math.round(a[i] + (b[i] - a[i]) * t)) as Rgb;

// The colour, darkened only as far as the muted text needs, keeping its hue.
export function readableStop(c: Rgb): Rgb {
  const muted = fromHex(THEME_MUTED) as Rgb;
  if (contrast(c, muted) >= AA_TEXT) return c;
  let [low, high] = [0, 1];
  for (let i = 0; i < 12; i++) {
    const f = (low + high) / 2;
    if (contrast(scale(c, f), muted) >= AA_TEXT) low = f;
    else high = f;
  }
  return scale(c, low);
}

// The two gradient stops. Luminance is convex along a straight line between two sRGB colours, so a point between two
// stops that pass is never lighter than the lighter of them, and the whole gradient passes.
export function gradientStops(theme: ProfileTheme): { top: Rgb; bottom: Rgb } {
  return { top: readableStop(theme.primary), bottom: readableStop(theme.accent) };
}

// The props of the element a theme fills: its tokens and its colours. Nothing for no theme, so that element looks as it
// does today.
export function themeProps(raw: unknown): { 'data-profile-theme'?: ''; style?: CSSProperties } {
  const theme = validTheme(raw);
  if (!theme) return {};
  const { top, bottom } = gradientStops(theme);
  const white: Rgb = [255, 255, 255];
  return {
    'data-profile-theme': '',
    style: {
      '--profile-top': cssRgb(top),
      '--profile-bottom': cssRgb(bottom),
      // The avatar's ring and the presence dot's: where the avatar sits, a quarter of the way down.
      '--profile-ring': cssRgb(mix(top, bottom, 0.25)),
      // The border follows the gradient in the colours as picked, lifted so a dark one still shows.
      '--profile-edge-top': cssRgb(mix(theme.primary, white, 0.3)),
      '--profile-edge-bottom': cssRgb(mix(theme.accent, white, 0.3)),
    } as CSSProperties,
  };
}
