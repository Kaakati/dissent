// Message density and font scale. Stored in localStorage (guarded) and applied through the root element: the density
// is a data attribute that index.css turns into tokens, and the scale is --ui-zoom. Ported from appearance.js.
const KEY = 'dissent.appearance.v1';

export type Density = 'cozy' | 'compact';
export type FontScale = 90 | 100 | 110;

export interface Appearance {
  density: Density;
  scale: FontScale;
}

const DEFAULTS: Appearance = { density: 'cozy', scale: 100 };

export function getAppearance(): Appearance {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    return { ...DEFAULTS, ...(saved && typeof saved === 'object' ? saved : {}) };
  } catch {
    return { ...DEFAULTS };
  }
}

// Zoom on the body scales every size, including px values, which CSS variables alone cannot do (shell.css).
export function applyAppearance(a: Appearance = getAppearance()): void {
  const root = document.documentElement;
  root.dataset.density = a.density === 'compact' ? 'compact' : 'cozy';
  root.style.setProperty('--ui-zoom', String([90, 100, 110].includes(a.scale) ? a.scale / 100 : 1));
}

export function setAppearance(next: Partial<Appearance>): Appearance {
  const a = { ...getAppearance(), ...next };
  try {
    window.localStorage.setItem(KEY, JSON.stringify(a));
  } catch {
    // Unavailable: the choice lasts for this session only.
  }
  applyAppearance(a);
  return a;
}
