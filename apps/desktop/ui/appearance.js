// Appearance: message density and font scale. Stored in localStorage (guarded), applied through CSS.
const KEY = 'dissent.appearance.v1';
const DEFAULTS = { density: 'cozy', scale: 100 };

export function getAppearance() {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    return { ...DEFAULTS, ...(saved && typeof saved === 'object' ? saved : {}) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function applyAppearance(a = getAppearance()) {
  const root = document.documentElement;
  root.dataset.density = a.density === 'compact' ? 'compact' : 'cozy';
  // Zoom on <body> scales every size, including px values, which CSS variables alone cannot do.
  root.style.setProperty('--ui-zoom', String([90, 100, 110].includes(a.scale) ? a.scale / 100 : 1));
}

export function setAppearance(next) {
  const a = { ...getAppearance(), ...next };
  try {
    window.localStorage.setItem(KEY, JSON.stringify(a));
  } catch {
    // Unavailable: the choice lasts for this session only.
  }
  applyAppearance(a);
  return a;
}
