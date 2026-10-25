// Full-pane pages that replace the chat list on Home: Transfers (table, storage). Friends lives in friends.js.
// Each builder returns { body, aside, title, titleIcon, headerExtra } nodes; chat.js places them in its header, list and right slots.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { api } from '../api.js';
import { el } from '../ui.js';
import { contactsPage } from './friends.js';

// ---- Transfers ----

const ROUTE_TAG = { direct: 'tag-ok', relay: 'tag-warn', server: 'tag-ok', seeder: 'tag-ok', waiting: 'tag-muted', refused: 'tag-bad', failed: 'tag-bad' };

function formatBytes(n) {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${n} B`;
}

const extOf = (name) => (name.includes('.') ? name.split('.').pop().toUpperCase() : 'FILE');

function transferActions(t) {
  const out = [];
  const btn = (label, iconName, fn) => out.push(el('button', { class: 'icon-btn round', 'aria-label': label, title: label, icon: iconName, onclick: fn }));
  if (t.state === 'active') {
    btn('Pause', 'pause', () => actions.pauseTransfer(t.id));
    btn('Cancel', 'x', () => actions.cancelTransfer(t.id));
  } else if (t.state === 'paused') {
    btn('Resume', 'play', () => actions.resumeTransfer(t.id));
    btn('Cancel', 'x', () => actions.cancelTransfer(t.id));
  } else if (t.state === 'failed') {
    btn('Retry', 'rotate-ccw', () => actions.retryTransfer(t.id));
    btn('Remove from list', 'trash-2', () => actions.removeTransfer(t.id));
  } else if (t.state === 'done') {
    btn('Show in folder', 'folder', () => t.local_path && actions.revealAttachment(t.local_path));
    btn('Remove from list', 'trash-2', () => actions.removeTransfer(t.id));
  } else {
    btn('Cancel', 'x', () => actions.cancelTransfer(t.id));
  }
  return out;
}

// The fill scales with transform only; the value is a custom property, set through style.setProperty.
function progressBar(pct) {
  const fill = el('span', { class: 'progress-fill' });
  fill.style.setProperty('--p', String(pct / 100));
  return el('span', { class: 'progress thin', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(pct) }, fill);
}

function transferRow(t) {
  const where = t.where ?? { name: '', kind: 'server' };
  const pct = Math.round((Number(t.progress) || 0) * 100);
  const showBar = t.state === 'active' || t.state === 'paused';
  return el('div', { class: `trow${t.state === 'failed' ? ' failed' : ''}`, role: 'row' },
    el('div', { class: 'cell', role: 'cell' },
      el('span', { class: 'tile sm', text: extOf(t.name) }),
      el('span', { class: 'stack' }, el('span', { class: 'name', text: t.name }), el('span', { class: 'sz', text: formatBytes(t.size) }))),
    el('div', { class: 'cell', role: 'cell' },
      el('span', { class: `avatar where-ic${where.kind === 'direct' ? '' : ' square'}`, text: (where.name || '?').slice(0, 2).toUpperCase() }),
      el('span', { class: 'stack' }, el('span', { class: 'where', text: where.name }), el('span', { class: 'kind', text: where.kind === 'direct' ? 'Direct message' : 'Server channel' }))),
    el('div', { class: 'cell dir', role: 'cell', text: t.direction === 'upload' ? '↑ Upload' : '↓ Download' }),
    el('div', { class: 'cell col', role: 'cell' },
      el('span', { class: `route-tag ${ROUTE_TAG[t.route_kind] ?? 'tag-muted'}`, text: t.route }),
      el('span', { class: 'note', text: t.route_note ?? '' })),
    el('div', { class: 'cell col', role: 'cell' },
      showBar ? progressBar(pct) : null,
      el('span', { class: 'status', text: t.status_text ?? '' })),
    el('div', { class: 'cell end', role: 'cell' }, transferActions(t)));
}

// Which tab the Transfers page shows. Kept across repaints of the page.
let transferTab = 'active';
const TRANSFER_TABS = [
  { key: 'active', label: 'Active', empty: 'No active transfers.' },
  { key: 'done', label: 'Completed', empty: 'No completed transfers.' },
  { key: 'failed', label: 'Failed', empty: 'No failed transfers.' },
];

export function transfersPage() {
  const rows = (state.transfers ?? []);
  const groups = {
    active: rows.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)),
    done: rows.filter((t) => t.state === 'done'),
    failed: rows.filter((t) => t.state === 'failed'),
  };
  const head = el('div', { class: 'trow head', role: 'row' },
    el('span', { role: 'columnheader', text: 'File' }), el('span', { role: 'columnheader', text: 'Shared in' }), el('span', { role: 'columnheader', text: 'Direction' }),
    el('span', { role: 'columnheader', text: 'Route' }), el('span', { role: 'columnheader', text: 'Progress' }), el('span', { role: 'columnheader', text: 'Actions' }));
  const table = el('div', { class: 'ttable', role: 'table', 'aria-label': 'Transfers' });
  const tabButtons = TRANSFER_TABS.map((tab) => el('button', {
    class: 'tx-tab', role: 'tab', type: 'button',
    'aria-selected': 'false',
    onclick: () => { transferTab = tab.key; paint(); },
  }, el('span', { text: tab.label }), el('span', { class: 'tx-count tnum', text: String(groups[tab.key].length) })));
  const tablist = el('div', { class: 'transfer-tabs', role: 'tablist', 'aria-label': 'Transfer state' }, ...tabButtons);
  const paint = () => {
    TRANSFER_TABS.forEach((tab, i) => {
      const on = tab.key === transferTab;
      tabButtons[i].setAttribute('aria-selected', String(on));
      tabButtons[i].classList.toggle('on', on);
    });
    const current = TRANSFER_TABS.find((tab) => tab.key === transferTab);
    const list = groups[transferTab];
    table.replaceChildren(head, ...(list.length ? list.map(transferRow) : [el('p', { class: 'page-empty', text: current.empty })]));
  };
  paint();
  const body = el('div', { class: 'page-scroll' }, table);
  // The tabs sit in the page header, as on the Transfers board.
  return { body, aside: storageAside(), title: 'Transfers', titleIcon: 'arrow-up-down', headerExtra: [tablist] };
}

function storageAside() {
  const settings = state.transferSettings ?? null;
  const max = settings?.max_accept_bytes ?? 10 * 1024 ** 3;
  const change = el('button', {
    class: 'ghost',
    text: 'Change',
    disabled: !settings,
    onclick: async () => {
      const dir = await actions.pickDownloadDir();
      if (dir && settings) {
        await actions.setTransferSettings({ ...settings, download_dir: dir });
        await actions.loadTransfers();
      }
    },
  });
  return el('aside', { class: 'page-aside', 'aria-label': 'Storage' },
    el('h3', { text: 'Storage' }),
    el('p', { text: 'Partial and cancelled files are cleared within the hour.' }),
    storeMeter(),
    el('div', { class: 'quiet-box' }, el('div', { class: 't', text: 'Largest file you’ll accept' }), el('div', { class: 's', text: `${formatBytes(max)}. Bigger files are refused before any data moves.` })),
    settings && 'auto_accept_bytes' in settings ? autoAcceptNodes(settings) : null,
    el('div', { class: 'dl-dir' },
      el('h4', { text: 'Save downloads to' }),
      el('div', { class: 'dl-path mono', text: settings?.download_dir ?? 'Not set' }),
      change));
}

// Auto-accept: files up to this size download without asking. Shown only when the backend reports the setting.
const AUTO_ACCEPT_OPTIONS = [
  { bytes: 25 * 1024 ** 2, label: '25 MB' },
  { bytes: 50 * 1024 ** 2, label: '50 MB' },
  { bytes: 100 * 1024 ** 2, label: '100 MB' },
  { bytes: null, label: 'Always ask' },
];

// The encrypted file store: used bytes against the budget, from store_stats. Empty until the backend answers.
function storeMeter() {
  const host = el('div', { class: 'store-box' });
  api.storeStats().then((s) => {
    const pct = s.budget_bytes ? Math.min(100, (s.used_bytes / s.budget_bytes) * 100) : 0;
    const parts = [
      ['Files you shared', s.shared_bytes, '#d2452f'],
      ['Files you downloaded', s.downloaded_bytes, '#4f8cff'],
      ['Partial and cancelled', s.partial_bytes, '#6b6f78'],
    ].filter(([, bytes]) => typeof bytes === 'number');
    const bar = el('div', { class: 'store-bar', role: 'meter', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(Math.round(pct)), 'aria-label': 'Encrypted file store' },
      parts.length
        ? parts.map(([, bytes, color]) => { const seg = el('span', { class: 'store-seg' }); seg.style.width = `${s.budget_bytes ? (bytes / s.budget_bytes) * 100 : 0}%`; seg.style.background = color; return seg; })
        : [el('span', { class: 'store-fill' })]);
    if (!parts.length) bar.firstChild.style.setProperty('--p', String(pct / 100));
    host.replaceChildren(
      el('div', { class: 'store-row' }, el('span', { text: 'Encrypted file store' }), el('span', { class: 'tnum', text: formatBytes(s.used_bytes) })),
      bar,
      parts.length ? el('ul', { class: 'store-legend' }, parts.map(([label, bytes, color]) => el('li', {},
        el('span', { class: 'swatch' }), el('span', { text: label }), el('span', { class: 'tnum', text: formatBytes(bytes) })))) : null);
    host.querySelectorAll('.swatch').forEach((sw, i) => { sw.style.background = parts[i][2]; });
  }).catch(() => {});
  return host;
}

function autoAcceptNodes(settings) {
  const buttons = AUTO_ACCEPT_OPTIONS.map((opt) => el('button', {
    class: 'ghost accept-opt',
    'aria-pressed': String((settings.auto_accept_bytes ?? null) === opt.bytes),
    text: opt.label,
    onclick: async () => {
      await actions.setTransferSettings({ ...settings, auto_accept_bytes: opt.bytes });
      await actions.loadTransfers();
    },
  }));
  return el('div', { class: 'auto-accept' },
    el('h4', { text: 'Auto-accept files up to' }),
    el('div', { class: 'accept-row', role: 'group', 'aria-label': 'Auto-accept files up to' }, ...buttons));
}

export const pageBuilders = { contacts: contactsPage, transfers: transfersPage };
