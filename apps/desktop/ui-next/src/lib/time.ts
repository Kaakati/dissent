// Day dividers and clock labels. 24-hour clock, relative day words for the last week. Ported from ui/ui.js.

const DAY_MS = 24 * 60 * 60 * 1000;
const startOfDay = (ms: number): number => {
  const d = new Date(Number(ms));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

export function dayKey(ms: number): string {
  const d = new Date(Number(ms));
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

// "Today", "Yesterday", "Thursday, October 22" within the year, else "October 21, 2025".
export function dayLabel(ms: number): string {
  const days = Math.round((startOfDay(Date.now()) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  const d = new Date(Number(ms));
  if (d.getFullYear() === new Date().getFullYear()) return d.toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' });
  return d.toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' });
}

export function clockLabel(ms: number): string {
  return new Date(Number(ms)).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

// "Today 09:14", "Yesterday 22:40", "Thu 18:02" within six days, else "10/21/2026 17:02".
export function whenLabel(ms: number): string {
  const days = Math.round((startOfDay(Date.now()) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return `Today ${clockLabel(ms)}`;
  if (days === 1) return `Yesterday ${clockLabel(ms)}`;
  if (days > 1 && days < 7) return `${new Date(Number(ms)).toLocaleDateString('en-US', { weekday: 'short' })} ${clockLabel(ms)}`;
  return stampLabel(ms);
}

export function stampLabel(ms: number): string {
  const d = new Date(Number(ms));
  return `${d.toLocaleDateString('en-US', { month: '2-digit', day: '2-digit', year: 'numeric' })} ${clockLabel(ms)}`;
}
