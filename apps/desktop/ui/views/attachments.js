// File attachments: staged chips above the composer, cards and thumbnails in messages, and the drop overlay.
// Nothing here renders user-supplied markup. Names go through textContent, and the only image source is the
// backend's local path converted by Tauri's asset protocol, never a URL taken from a message.
import { has } from '../features.js';
import { el, icon } from '../ui.js';
import * as actions from '../actions.js';
import { confirmDialog } from './dialogs.js';

// The hard cap on one attachment. Every limit message below is derived from it.
export const MAX_FILE_BYTES = 10 * 1024 ** 3;
export const MAX_FILE_LABEL = '10 GB';
const EXECUTABLE = new Set(['exe', 'msi', 'bat', 'cmd', 'ps1', 'js', 'vbs', 'scr', 'jar', 'app', 'dmg', 'sh']);
const ARCHIVE = new Set(['zip', '7z', 'rar', 'gz', 'tgz', 'tar', 'xz', 'bz2']);
const CODE = new Set(['js', 'mjs', 'ts', 'rs', 'py', 'json', 'html', 'css', 'toml', 'yaml', 'yml', 'sh', 'c', 'h', 'cpp', 'go', 'java']);
const TEXT = new Set(['md', 'txt', 'pdf', 'doc', 'docx', 'rtf', 'csv', 'log']);
const MIME_BY_EXT = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
  mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', mp4: 'video/mp4', mkv: 'video/x-matroska', webm: 'video/webm',
  pdf: 'application/pdf', zip: 'application/zip', txt: 'text/plain', md: 'text/markdown', csv: 'text/csv', json: 'application/json',
};

export function formatSize(bytes) {
  if (bytes === null || bytes === undefined || Number.isNaN(Number(bytes))) return 'size unknown';
  const n = Number(bytes);
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

// The real extension: the part after the last dot, so "invoice.pdf.exe" reports "exe".
export function extOf(name) {
  const m = /\.([a-z0-9]{1,8})$/i.exec(String(name ?? '').trim());
  return m ? m[1].toLowerCase() : '';
}
export const isExecutable = (name) => EXECUTABLE.has(extOf(name));
export const isOverLimit = (file) => file.size !== null && file.size !== undefined && Number(file.size) > MAX_FILE_BYTES;
export const baseName = (path) => String(path ?? '').split(/[\\/]/).pop() || 'file';
export const guessMime = (name) => MIME_BY_EXT[extOf(name)] ?? 'application/octet-stream';

// SVG is never treated as an image: it stays a file card.
export function isImage(a) {
  return typeof a.mime === 'string' && a.mime.startsWith('image/') && !a.mime.includes('svg');
}

export function iconFor(a) {
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

// Only the backend's local path becomes an image source, through Tauri's asset protocol.
function imageSrc(a) {
  const convert = window.__TAURI__?.core?.convertFileSrc;
  if (!a.local_path || typeof convert !== 'function') return null;
  const src = convert(a.local_path);
  return typeof src === 'string' ? src : null;
}

const lockGlyph = () => el('span', { class: 'att-lock', title: 'End-to-end encrypted', 'aria-label': 'End-to-end encrypted', role: 'img' }, icon('lock'));

// ---- in messages ----

// ctx: { messageId }
// Rates from 1 MB/s up read in MB/s, slower ones in KB/s, as the FilesServer board shows.
export function formatRate(bps) {
  return bps >= 1024 ** 2 ? `${(bps / 1024 ** 2).toFixed(1)} MB/s` : `${formatSize(bps)}/s`;
}

// One cell per piece up to 96 cells, then the verified share of 96. A 1,383-piece file still fits the panel.
export function pieceCells(total, verified) {
  const n = Math.min(total, 96);
  const filled = total ? Math.round((verified / total) * n) : 0;
  return Array.from({ length: n }, (_, i) => el('span', { class: i < filled ? 'ok' : '' }));
}

export function attachmentNode(a, ctx) {
  const name = String(a.name ?? 'file');
  const src = isImage(a) && a.state === 'downloaded' ? imageSrc(a) : null;
  if (src) {
    return el('figure', { class: 'att-image' },
      el('button', {
        class: 'att-image-btn',
        'aria-label': `Open ${name}`,
        title: `${name} · ${formatSize(a.size)} · End-to-end encrypted`,
        onclick: () => openFile(a),
      }, el('img', { src, alt: name, loading: 'lazy', decoding: 'async', draggable: false })),
      lockGlyph());
  }
  const ext = extOf(name);
  const exec = isExecutable(name);
  return el('div', { class: `att-card${a.state === 'failed' ? ' failed' : ''}${a.state === 'downloading' ? ' downloading' : ''}`, 'data-att': a.id },
    el('span', { class: 'att-icon' }, icon(iconFor(a))),
    el('div', { class: 'att-body' },
      el('div', { class: 'att-name', title: name },
        el('span', { class: 'att-name-text', text: name, ...(a.state === 'downloading' ? { title: 'Transfer details', onclick: () => actions.openFileDetail(a.id) } : {}) }),
        ext ? el('span', { class: `att-ext${exec ? ' danger' : ''}`, text: ext.toUpperCase(), title: exec ? 'This is a program' : `.${ext} file` }) : null),
      el('div', { class: 'att-meta' }, el('span', { class: 'tnum', text: formatSize(a.size) }), lockGlyph(), stateNode(a)),
      a.state === 'downloading' || a.state === 'paused' || a.state === 'sending' ? progressBar(a.progress) : null,
      a.state === 'waiting' && a.note ? el('div', { class: 'piece-note', text: a.note }) : null,
      a.state === 'downloading' && Array.isArray(a.sources) && a.sources.length > 1 ? el('div', { class: 'att-sources', text: `From ${a.sources.length} sources` }) : null),
    ...cardActions(a, ctx));
}

// Transferred and total for long downloads: "1.2 GB of 9.8 GB · 12%".
function transferText(a) {
  const p = Math.max(0, Math.min(1, Number(a.progress) || 0));
  const pct = `${Math.round(p * 100)}%`;
  return a.size ? `${formatSize(p * Number(a.size))} of ${formatSize(a.size)} · ${pct}` : pct;
}

function stateNode(a) {
  switch (a.state) {
    case 'downloading': return el('span', { class: 'att-state tnum', text: transferText(a) });
    case 'paused': return el('span', { class: 'att-state tnum', text: `Paused · ${transferText(a)}` });
    case 'failed': return el('span', { class: 'att-state danger', text: 'Download failed' });
    case 'disk_full': return el('span', { class: 'att-state danger tnum', text: `Not enough disk space${a.needed ? ` (needs ${formatSize(a.needed)})` : ''}` });
    case 'too_large': return el('span', { class: 'att-state', text: `Too large to download (over ${MAX_FILE_LABEL})` });
    case 'waiting': return el('span', { class: 'att-state', text: 'Waiting for a source' });
    case 'sending': return el('span', { class: 'att-state tnum', text: a.status ?? 'Sending' });
    case 'downloaded': return a.verified ? el('span', { class: 'att-state', style: 'color: var(--success-text)', text: 'Saved to Downloads · matches the fingerprint' }) : null;
    default: return null;
  }
}

function progressBar(progress) {
  const fill = el('div', { class: 'progress-fill' });
  fill.style.setProperty('--p', String(Math.max(0, Math.min(1, Number(progress) || 0))));
  return el('div', { class: 'progress att-progress', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(Math.round((Number(progress) || 0) * 100)) }, fill);
}

function cardActions(a, ctx) {
  const btn = (name, label, onclick, cls = '') => el('button', { class: `icon-btn${cls}`, 'aria-label': label, title: label, icon: name, onclick });
  const download = () => actions.downloadAttachment(ctx.messageId, a.id);
  switch (a.state) {
    case 'available': return [btn('download', `Download ${a.name}`, download)];
    case 'downloading':
    case 'sending': return a.transfer_id ? [
      btn('pause', `Pause ${a.name}`, () => actions.pauseTransfer(a.transfer_id)),
      btn('x', `Cancel ${a.name}`, () => actions.cancelTransfer(a.transfer_id)),
    ] : [];
    case 'paused': return [btn('download', `Resume download of ${a.name}`, download)];
    case 'failed':
    case 'disk_full': return [btn('rotate-ccw', `Retry download of ${a.name}`, download)];
    case 'downloaded': return [
      btn('external-link', `Open ${a.name}`, () => openFile(a)),
      btn('folder-open', `Show ${a.name} in folder`, () => actions.revealAttachment(a.local_path)),
    ];
    default: return [];
  }
}

// Programs get a confirmation first. The check uses the real (last) extension.
export async function openFile(a) {
  if (!a.local_path) return;
  if (isExecutable(a.name)) {
    const ok = await confirmDialog(`"${a.name}" is a program (.${extOf(a.name)}). Opening it runs it on this computer. Only open programs from people you trust.`, 'Open anyway');
    if (!ok) return;
  }
  await actions.openAttachment(a.local_path);
}

// ---- staged files above the composer ----

export function stagedChip(file, onRemove) {
  const over = isOverLimit(file);
  const name = String(file.name ?? baseName(file.path));
  return el('div', { class: `chip-file${over ? ' danger' : ''}`, title: over ? `${name} is over ${MAX_FILE_LABEL} and cannot be sent` : name },
    icon(iconFor(file)),
    el('span', { class: 'chip-name', text: name }),
    el('span', { class: 'chip-size tnum', text: over ? `over ${MAX_FILE_LABEL}` : formatSize(file.size) }),
    el('button', { class: 'icon-btn', 'aria-label': `Remove ${name}`, title: 'Remove', icon: 'x', onclick: onRemove }));
}

// ---- drop overlay ----

export function dropOverlay() {
  const title = el('p', { class: 'drop-title' });
  const node = el('div', { class: 'drop-overlay', 'aria-hidden': 'true' },
    el('div', { class: 'drop-box' }, icon('paperclip'), title, el('p', { class: 'muted', text: `Up to ${MAX_FILE_LABEL}, end-to-end encrypted` })));
  node.setTitle = (text) => { title.textContent = text; };
  return node;
}
