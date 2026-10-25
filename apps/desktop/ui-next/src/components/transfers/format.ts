// Byte and rate labels for the transfer views. Ported from views/pages.js (formatBytes) and views/attachments.js
// (formatSize, formatRate); the thresholds and the one-decimal rounding are the same.

export function formatBytes(n: number): string {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${n} B`;
}

export function formatSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(Number(bytes))) return 'size unknown';
  return formatBytes(Number(bytes));
}

export function formatRate(bps: number): string {
  return bps >= 1024 ** 2 ? `${(bps / 1024 ** 2).toFixed(1)} MB/s` : `${formatSize(bps)}/s`;
}
