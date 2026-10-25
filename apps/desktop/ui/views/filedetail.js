// File details in the right column (FilesServer and FilesDirect boards): one transfer, its sources, its verified pieces,
// its fingerprint, and where it is shared. Opened from the Details button on a downloading card (Part 6.7).
import { state, notify } from '../state.js';
import { el, icon, initials, withImage } from '../ui.js';
import * as actions from '../actions.js';
import { formatSize, formatRate, pieceCells } from './attachments.js';

const ROUTE_LABEL = { direct: 'Direct', relay: 'Via relay', seeder: 'Seeder' };

export function fileDetailNodes() {
  const id = state.ui.fileDetail;
  if (!id) return [];
  const found = [...state.messages, ...state.dmMessages].map((m) => (m.attachments ?? []).find((a) => a.id === id)).find(Boolean);
  if (!found) return [];
  if (found.direct) return directTransferNodes(found);
  const sources = Array.isArray(found.sources) ? found.sources : [];
  const total = Math.max(0, Number(found.pieces?.total) || 0);
  const verified = Math.min(total, Math.max(0, Number(found.pieces?.verified) || 0));
  const direct = state.route.kind === 'home';
  const size = Number(found.size) || 0;
  const progress = Math.min(1, Math.max(0, Number(found.progress) || 0));
  const rate = sources.reduce((sum, src) => sum + (Number(src.rate_bps) || 0), 0);
  const left = rate > 0 ? Math.round((size * (1 - progress)) / rate) : null;
  const pct = Math.round(progress * 100);
  const meta = `${formatSize(size * progress)} of ${formatSize(size)}`
    + (rate > 0 ? ` · ${formatSize(rate)}/s` : '')
    + (left !== null ? ` · about ${left} s left` : '');
  const close = el('button', { class: 'icon-btn', 'aria-label': 'Close transfer details', icon: 'x', onclick: () => { state.ui.fileDetail = null; notify('right'); } });
  return [el('div', { class: 'file-detail', 'aria-label': 'Transfer details' },
    el('div', { class: 'fd-head' }, el('h3', { text: 'Transfer' }), close),
    el('div', { class: 'fd-name', text: found.name }),
    el('div', { class: 'fd-meta tnum', text: meta }),
    el('div', { class: 'progress fd-progress', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(pct) }, progressFill(pct)),
    el('div', { class: 'fd-shared' }, icon('lock'), el('span', {},
      el('strong', { text: direct ? 'Sent directly' : 'Shared in a server' }), el('br'),
      el('span', { text: found.shared_in ?? (direct ? 'Only this conversation gets the key.' : 'Only its members get the key.') }))),
    el('h4', { text: sources.length === 1 ? 'Downloading directly' : 'Downloading from ' + sources.length + ' sources' }),
    el('div', { class: 'fd-sources' }, sources.map((src) => el('div', { class: 'fd-source' },
      el('span', { class: 'fd-src-who' },
        el('span', { class: 'fd-src-name', text: src.display_name ?? 'Unknown' }),
        src.note ? el('span', { class: 'fd-note', text: src.note }) : null),
      el('span', { class: 'fd-src-route', text: ROUTE_LABEL[src.route] ?? 'Direct' }),
      el('span', { class: 'fd-src-rate tnum', text: src.rate_bps ? formatRate(src.rate_bps) : '' })))),
    el('h4', { text: 'Checked as it arrives' }),
    total ? el('div', { class: 'pieces' }, pieceCells(total, verified)) : null,
    el('div', { class: 'fd-note tnum', text: verified.toLocaleString('en-US') + ' of ' + total.toLocaleString('en-US') + ' pieces verified against the file’s fingerprint' }),
    found.transfer_id ? el('div', { class: 'dt-actions' },
      el('button', { class: 'ghost', text: 'Pause', onclick: () => actions.pauseTransfer(found.transfer_id) }),
      el('button', { class: 'ghost', text: 'Cancel', onclick: () => actions.cancelTransfer(found.transfer_id) })) : null,
    el('dl', { class: 'kv fd-kv' },
      found.fingerprint ? el('dt', { text: 'Fingerprint' }) : null,
      found.fingerprint ? el('dd', { class: 'mono', text: found.fingerprint }) : null,
      el('dt', { text: 'Encryption' }), el('dd', { text: direct ? 'Its own key, sent sealed to this conversation only' : 'Its own key, sent sealed to members only' }),
      el('dt', { text: direct ? 'Relay' : 'Relay, seeder' }), el('dd', { text: 'See encrypted bytes and size, never the name or content' }),
      el('dt', { text: 'Playback' }), el('dd', { text: 'Opens when the whole file has arrived' }))),
  ];
}

// A direct peer-to-peer transfer in a DM (FilesDirect board): the two devices, the path, the speed and the stop controls.
function directTransferNodes(found) {
  const d = found.direct;
  const close = el('button', { class: 'icon-btn', 'aria-label': 'Close transfer details', icon: 'x', onclick: () => { state.ui.fileDetail = null; notify('right'); } });
  const stat = (label, value) => el('div', { class: 'dt-stat' }, el('span', { class: 'dt-label', text: label }), el('span', { class: 'dt-value tnum', text: value }));
  const sent = (d.sent / 1024 ** 2).toFixed(1) + ' / ' + formatSize(d.total);
  const pct = Math.round((Number(found.progress) || 0) * 100);
  const transferId = found.transfer_id;
  return [el('div', { class: 'file-detail direct-transfer', 'aria-label': 'Transfer details' },
    el('div', { class: 'fd-head' }, el('h3', { text: 'Direct transfer' }), close),
    el('div', { class: 'dt-peers' },
      el('span', { class: 'dt-peer' }, withImage(el('span', { class: 'dt-initials', text: initials(state.me?.display_name) }), state.me?.root_id), el('span', { text: 'You' })),
      el('span', { class: 'dt-line', 'aria-hidden': 'true' }),
      el('span', { class: 'dt-peer' }, el('span', { class: 'dt-initials', text: d.peer.slice(0, 2).toUpperCase() }), el('span', { text: d.peer }))),
    el('h4', { class: 'dt-title', text: `Straight to ${d.peer}’s device` }),
    el('p', { class: 'muted dt-lede', text: 'Your two devices found each other and connected directly, so nothing passes through anyone else.' }),
    el('div', { class: 'dt-stats' },
      stat('Path', 'Direct'),
      stat('Speed', formatSize(d.speed_bps) + '/s'),
      stat('Sent', sent),
      stat('Time left', '~' + d.time_left_s + ' s')),
    el('div', { class: 'progress fd-progress', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(pct) }, progressFill(pct)),
    el('dl', { class: 'kv fd-kv' },
      el('dt', { text: 'Stored on' }), el('dd', { text: `Your device and ${d.peer}’s. No server keeps a copy.` }),
      el('dt', { text: 'Encryption' }), el('dd', { text: `The file has its own key. Only ${d.peer} receives it.` }),
      el('dt', { text: 'If it stalls' }), el('dd', { text: 'After 30 s of silence Dissent reconnects and carries on.' })),
    transferId ? el('div', { class: 'dt-actions' },
      el('button', { class: 'ghost', text: 'Pause', onclick: () => actions.pauseTransfer(transferId) }),
      el('button', { class: 'ghost danger', text: 'Cancel', onclick: () => actions.cancelTransfer(transferId) })) : null)];
}

function progressFill(pct) {
  const fill = el('span', { class: 'progress-fill' });
  fill.style.setProperty('--p', String(pct / 100));
  return fill;
}
