// Unit tests for the profile colours: readable on any pick, and nothing but validated numbers reaches the CSS. Run with
// `npm run test:unit` (node --test, no build step).
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { contrast, fromHex, gradientStops, readableStop, themeProps, THEME_MUTED, THEME_TEXT, toHex, validTheme, type Rgb } from '../src/lib/profile-theme.ts';

const WHITE: Rgb = [255, 255, 255];
const YELLOW: Rgb = [255, 255, 0];
const AA = 4.5;

const lerp = (a: Rgb, b: Rgb, t: number): Rgb => [0, 1, 2].map((i) => Math.round(a[i] + (b[i] - a[i]) * t)) as Rgb;

test('text passes AA across the whole gradient on any pair of colours', () => {
  const channel = [0, 64, 128, 200, 255];
  const colours: Rgb[] = [];
  for (const r of channel) for (const g of channel) for (const b of channel) colours.push([r, g, b]);
  const text = fromHex(THEME_TEXT) as Rgb;
  const muted = fromHex(THEME_MUTED) as Rgb;
  for (const primary of colours) {
    for (const accent of [WHITE, YELLOW, [0, 0, 0], [255, 0, 0], primary] as Rgb[]) {
      const { top, bottom } = gradientStops({ primary, accent });
      for (let t = 0; t <= 1; t += 0.125) {
        const at = lerp(top, bottom, t);
        assert.ok(contrast(at, muted) >= AA, `muted on ${at} (${primary} to ${accent})`);
        assert.ok(contrast(at, text) >= AA, `name on ${at} (${primary} to ${accent})`);
      }
    }
  }
});

test('a colour that already passes is kept, and a bright one keeps its hue', () => {
  assert.deepEqual(readableStop([10, 20, 90]), [10, 20, 90]);
  const [r, g, b] = readableStop(YELLOW);
  assert.equal(b, 0);
  assert.equal(r, g);
  assert.ok(r > 60, `yellow stays a colour, not black: ${r}`);
  const grey = readableStop(WHITE);
  assert.ok(grey[0] === grey[1] && grey[1] === grey[2] && grey[0] > 60, `white stays a mid grey: ${grey}`);
});

test('the text colours are the ones the theme tokens use', () => {
  const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');
  assert.match(css, new RegExp(`--profile-text: ${THEME_TEXT};`));
  assert.match(css, new RegExp(`--profile-muted: ${THEME_MUTED};`));
});

test('anything but two RGB byte triples is no theme, so a peer cannot reach the CSS', () => {
  const good = [1, 2, 3];
  for (const bad of [
    null,
    undefined,
    'red',
    {},
    { primary: good },
    { primary: 'red;} body{display:none', accent: good },
    { primary: ['1);background:url(x)', 2, 3], accent: good },
    { primary: [1, 2], accent: good },
    { primary: [1, 2, 3, 4], accent: good },
    { primary: [256, 0, 0], accent: good },
    { primary: [-1, 0, 0], accent: good },
    { primary: [1.5, 0, 0], accent: good },
    { primary: ['1', '2', '3'], accent: good },
    { primary: { length: 3, 0: 1, 1: 2, 2: 3 }, accent: good },
  ]) {
    assert.equal(validTheme(bad), null, JSON.stringify(bad));
    assert.deepEqual(themeProps(bad), {}, JSON.stringify(bad));
  }
  assert.deepEqual(validTheme({ primary: good, accent: [255, 255, 255], extra: 'x' }), { primary: good, accent: WHITE });
});

test('the style holds only rgb() of numbers', () => {
  const { style, 'data-profile-theme': mark } = themeProps({ primary: YELLOW, accent: WHITE });
  assert.equal(mark, '');
  for (const value of Object.values(style as Record<string, string>)) assert.match(value, /^rgb\(\d{1,3} \d{1,3} \d{1,3}\)$/);
});

test('hex for the colour inputs round-trips, and a bad one is refused', () => {
  assert.equal(toHex([255, 0, 10]), '#ff000a');
  assert.deepEqual(fromHex('#FF000A'), [255, 0, 10]);
  assert.equal(fromHex('#ff00'), null);
  assert.equal(fromHex('red'), null);
});
