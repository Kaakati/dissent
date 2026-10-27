// Day dividers and clock labels. A 12-hour clock with AM and PM, like Discord, and relative day words for the last week.
// Every message-related time goes through the helpers here, so the format is set in one place. Ported from the old UI's ui.js.

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

const clockFormat = new Intl.DateTimeFormat('en-US', { hour: 'numeric', minute: '2-digit', hour12: true });

// "5:02 PM". The space before AM or PM is a plain space, not the narrow no-break space the browser's formatter uses.
export function clockLabel(ms: number): string {
  return clockFormat.format(new Date(Number(ms))).replace(/\u202f/g, ' ');
}

// "Today 9:14 AM", "Yesterday 10:40 PM", "Thu 6:02 PM" within six days, else "10/21/2026 5:02 PM".
export function whenLabel(ms: number): string {
  const days = Math.round((startOfDay(Date.now()) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return `Today ${clockLabel(ms)}`;
  if (days === 1) return `Yesterday ${clockLabel(ms)}`;
  if (days > 1 && days < 7) return `${new Date(Number(ms)).toLocaleDateString('en-US', { weekday: 'short' })} ${clockLabel(ms)}`;
  return stampLabel(ms);
}

// The group header's stamp, as the reference UI shows it: "10/21/2026 5:02 PM", a 12-hour clock with the date always.
export function headStampLabel(ms: number): string {
  const d = new Date(Number(ms));
  const date = d.toLocaleDateString('en-US', { month: '2-digit', day: '2-digit', year: 'numeric' });
  return `${date} ${clockLabel(ms)}`;
}

export function stampLabel(ms: number): string {
  const d = new Date(Number(ms));
  return `${d.toLocaleDateString('en-US', { month: '2-digit', day: '2-digit', year: 'numeric' })} ${clockLabel(ms)}`;
}
