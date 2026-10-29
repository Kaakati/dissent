// File rules for the composer and the message cards: size limit, names, extensions and icons. Ported from
// the old UI's views/attachments.js. Nothing here renders markup; the icon is named, and the component maps it to a glyph.

// The hard cap on one attachment. Every limit message is derived from it.
export const MAX_FILE_BYTES = 10 * 1024 ** 3;
export const MAX_FILE_LABEL = '10 GB';

const EXECUTABLE = new Set(['exe', 'msi', 'bat', 'cmd', 'ps1', 'js', 'vbs', 'scr', 'jar', 'app', 'dmg', 'sh']);
const ARCHIVE = new Set(['zip', '7z', 'rar', 'gz', 'tgz', 'tar', 'xz', 'bz2']);
const CODE = new Set(['js', 'mjs', 'ts', 'rs', 'py', 'json', 'html', 'css', 'toml', 'yaml', 'yml', 'sh', 'c', 'h', 'cpp', 'go', 'java']);
const TEXT = new Set(['md', 'txt', 'pdf', 'doc', 'docx', 'rtf', 'csv', 'log']);
const MIME_BY_EXT: Record<string, string> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
  mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', mp4: 'video/mp4', mkv: 'video/x-matroska', webm: 'video/webm',
  pdf: 'application/pdf', zip: 'application/zip', txt: 'text/plain', md: 'text/markdown', csv: 'text/csv', json: 'application/json',
};

export type FileIconName = 'image' | 'file-audio' | 'file-video' | 'file-archive' | 'file-code' | 'file-text' | 'file';

export function formatSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(Number(bytes))) return 'size unknown';
  const n = Number(bytes);
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

// Rates from 1 MB/s up read in MB/s, slower ones in KB/s, as the FilesServer board shows.
export function formatRate(bps: number): string {
  return bps >= 1024 ** 2 ? `${(bps / 1024 ** 2).toFixed(1)} MB/s` : `${formatSize(bps)}/s`;
}

// The real extension: the part after the last dot, so "invoice.pdf.exe" reports "exe".
export function extOf(name: string | null | undefined): string {
  const m = /\.([a-z0-9]{1,8})$/i.exec(String(name ?? '').trim());
  return m ? m[1].toLowerCase() : '';
}

export const isExecutable = (name: string): boolean => EXECUTABLE.has(extOf(name));
export const isOverLimit = (file: { size: number | null }): boolean =>
  file.size !== null && file.size !== undefined && Number(file.size) > MAX_FILE_BYTES;
export const baseName = (path: string): string => String(path ?? '').split(/[\\/]/).pop() || 'file';
export const guessMime = (name: string): string => MIME_BY_EXT[extOf(name)] ?? 'application/octet-stream';

// SVG is never treated as an image: it stays a file card.
export function isImage(a: { mime?: string }): boolean {
  return typeof a.mime === 'string' && a.mime.startsWith('image/') && !a.mime.includes('svg');
}

// A video file. The engine sends its first frame as a thumbnail when it can decode it.
export function isVideo(a: { mime?: string }): boolean {
  return typeof a.mime === 'string' && a.mime.startsWith('video/');
}

export function iconFor(a: { mime?: string; name?: string }): FileIconName {
  const mime = String(a.mime ?? '');
  const ext = extOf(a.name);
  if (isImage(a)) return 'image';
  if (mime.startsWith('audio/')) return 'file-audio';
  if (mime.startsWith('video/')) return 'file-video';
  if (ARCHIVE.has(ext) || /zip|tar|gzip|7z|rar|compressed/.test(mime)) return 'file-archive';
  if (CODE.has(ext)) return 'file-code';
  if (TEXT.has(ext) || mime.startsWith('text/') || mime === 'application/pdf') return 'file-text';
  return 'file';
}

// Transferred and total for long downloads: "1.2 GB of 9.8 GB · 12%".
export function transferText(a: { progress?: number; size: number | null }): string {
  const p = Math.max(0, Math.min(1, Number(a.progress) || 0));
  const pct = `${Math.round(p * 100)}%`;
  return a.size ? `${formatSize(p * Number(a.size))} of ${formatSize(a.size)} · ${pct}` : pct;
}
