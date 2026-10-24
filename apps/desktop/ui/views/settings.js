// Full-page Settings. It takes over the window: a nav column on the sidebar surface, content on the chat surface.
// Esc or the close button returns to the window, and focus goes back to whatever opened it.
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';
import { api } from '../api.js';
import { el, icon, copyText, toast } from '../ui.js';
import { confirmDialog } from './dialogs.js';
import { cardContent } from './profile.js';
import { getAppearance, setAppearance } from '../appearance.js';
import { openUpdateDialog } from './updater.js';
import { has } from '../features.js';
import { recoveryNodes, devicesNodes } from './guardians.js';

// One group, in the board's order. Recovery and Devices appear only when the engine reports them.
const NAV = [
  { key: 'account', label: 'My identity', icon: 'user' },
  { key: 'recovery', label: 'Recovery', icon: 'key-round', feature: 'guardians' },
  { key: 'devices', label: 'Devices', icon: 'server', feature: 'devices' },
  { key: 'profile', label: 'Contacts & privacy', icon: 'shield-check' },
  { key: 'appearance', label: 'Appearance', icon: 'palette' },
  { key: 'network', label: 'Network', icon: 'wifi' },
  { key: 'updates', label: 'Updates', icon: 'download' },
];
const TITLES = Object.fromEntries(NAV.map((n) => [n.key, n.label]));
const NET_REFRESH_MS = 15000;

let page = null;
let opener = null;
let section = 'account';
let netTimer = null;
let navQuery = '';

function navItems() {
  const q = navQuery.trim().toLowerCase();
  return NAV.filter((n) => (!n.feature || has(n.feature)) && n.label.toLowerCase().includes(q));
}

export function openSettingsPage(which = 'account') {
  if (page) {
    show(which);
    return;
  }
  opener = document.activeElement;
  const search = el('input', {
    class: 'settings-search',
    type: 'search',
    placeholder: 'Search',
    'aria-label': 'Search settings',
    value: navQuery,
    oninput: (e) => {
      navQuery = e.target.value;
      renderNav();
    },
  });
  const nav = el('nav', { class: 'settings-nav', 'aria-label': 'Settings sections' },
    search,
    el('h2', { class: 'settings-nav-title', text: 'You' }),
    el('div', { class: 'settings-nav-list' }));
  const content = el('div', { class: 'settings-content', id: 'settings-content' });
  const close = el('button', { class: 'settings-close icon-btn', 'aria-label': 'Close settings', title: 'Close (Esc)', icon: 'x', onclick: closeSettingsPage });
  page = el('div', { class: 'settings-page', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Settings' },
    el('div', { class: 'settings-side' }, nav), content,
    el('div', { class: 'settings-close-col' }, close, el('span', { class: 'esc-label', text: 'ESC' })));
  document.body.append(page);
  document.addEventListener('keydown', onKey, true);
  show(which);
  page.querySelector('.settings-nav button[aria-current="true"]')?.focus();
}

export function closeSettingsPage() {
  if (!page) return;
  clearInterval(netTimer);
  netTimer = null;
  document.removeEventListener('keydown', onKey, true);
  page.remove();
  page = null;
  if (opener?.isConnected) opener.focus();
  opener = null;
}

function onKey(e) {
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    closeSettingsPage();
  }
}

// Profile preview follows display-name changes, which notify the sidebar.
subscribe('sidebar', () => {
  if (page && section === 'profile') show('profile');
});

// The nav filters by the search text. Only the items list is rebuilt, so the search field keeps focus.
function renderNav() {
  const items = navItems();
  page.querySelector('.settings-nav-list').replaceChildren(
    ...items.map((n) => el('button', {
      class: 'settings-nav-item',
      icon: n.icon,
      text: n.label,
      'aria-current': n.key === section ? 'true' : null,
      onclick: () => show(n.key),
    })),
    ...(items.length ? [] : [el('p', { class: 'muted settings-nav-empty', text: 'No matching settings' })]));
}

function show(which) {
  section = which;
  clearInterval(netTimer);
  netTimer = null;
  renderNav();
  const asyncSection = { recovery: () => recoveryNodes({ onCancel: closeSettingsPage }), devices: devicesNodes }[section];
  const host = el('div', { class: 'settings-async' }, asyncSection ? el('p', { class: 'muted', text: 'Loading…' }) : null);
  const body = asyncSection ? host : { account, profile, network, appearance, updates }[section]();
  page.querySelector('.settings-content').replaceChildren(
    el('header', { class: 'settings-head' }, el('h1', { text: TITLES[section] })),
    el('div', { class: asyncSection ? 'settings-body settings-body-async' : 'settings-body' }, body),
  );
  if (asyncSection) {
    const current = section;
    asyncSection().then((nodes) => {
      if (section === current) host.replaceChildren(...nodes);
    });
  }
}

// ---- My Account ----

function account() {
  const me = state.me;
  const nameInput = el('input', { value: me?.display_name ?? '', maxLength: 64, 'aria-label': 'Display name' });
  const saveBtn = el('button', {
    class: 'primary',
    text: 'Save',
    onclick: async () => {
      const name = nameInput.value.trim();
      if (!name) {
        toast('Display name cannot be empty');
        return;
      }
      await actions.setDisplayName(name);
    },
  });

  const phraseBox = el('textarea', { readOnly: true, rows: 3, hidden: true, 'aria-label': 'Recovery phrase' });
  const phraseWarning = el('p', { class: 'warning', hidden: true, text: 'Anyone with these 24 words controls your identity. Keep them offline and never share them.' });
  const phraseLabel = el('span', { text: 'Show recovery phrase' });
  const phraseBtn = el('button', { class: 'ghost', icon: 'key-round' }, phraseLabel);
  phraseBtn.addEventListener('click', async () => {
    if (!phraseBox.hidden) {
      phraseBox.hidden = true;
      phraseWarning.hidden = true;
      phraseBox.value = '';
      phraseLabel.textContent = 'Show recovery phrase';
      return;
    }
    const phrase = await actions.loadRecoveryPhrase();
    if (!phrase) return;
    phraseBox.value = phrase;
    phraseBox.hidden = false;
    phraseWarning.hidden = false;
    phraseLabel.textContent = 'Hide recovery phrase';
  });

  const importInput = el('textarea', { rows: 3, placeholder: '24 recovery words, separated by spaces', 'aria-label': 'Recovery phrase to import' });
  const importBtn = el('button', {
    class: 'danger-outline',
    icon: 'key-round',
    text: 'Import phrase',
    onclick: async () => {
      const phrase = importInput.value.trim();
      if (!phrase) return;
      if (!(await confirmDialog('Replace the identity on this device with the one from this phrase?', 'Import'))) return;
      if (await actions.restoreIdentity(phrase)) importInput.value = '';
    },
  });

  return el('div', { class: 'settings-card' },
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Display name' }),
      el('div', { class: 'row' }, nameInput, saveBtn)),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Identity' }),
      idRow('Root ID', me?.root_id ?? ''),
      idRow('Device ID', me?.device_id ?? '')),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Recovery phrase' }),
      el('div', { class: 'row' }, phraseBtn),
      phraseWarning,
      phraseBox),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Import recovery phrase' }),
      el('p', { class: 'muted', text: 'Restoring replaces the identity stored on this device.' }),
      importInput,
      el('div', { class: 'row end' }, importBtn)));
}

function idRow(label, value) {
  return el('div', { class: 'id-row' },
    el('span', { class: 'id-label', text: label }),
    el('code', { class: 'mono id-value', text: value }),
    el('button', {
      class: 'icon-btn',
      'aria-label': `Copy ${label}`,
      title: `Copy ${label}`,
      icon: 'copy',
      onclick: async () => {
        if (await copyText(value)) toast(`${label} copied`, 'info');
      },
    }));
}

// ---- Profile: the profile card as others see it ----

function profile() {
  const host = el('div', { class: 'card-preview-host' }, el('p', { class: 'muted', text: 'Loading preview…' }));
  const me = state.me;
  if (me) {
    actions.userProfile(me.root_id).then((view) => {
      if (!view || section !== 'profile') return;
      host.replaceChildren(el('div', { class: 'profile-card preview' }, cardContent(view, me.root_id)));
    });
  }
  return el('div', { class: 'settings-card' },
    el('p', { class: 'muted', text: 'This is your card as other people see it.' }),
    host);
}

// ---- Network: relay list, the public-relay toggle, and live status ----

function network() {
  let settings = { use_public_relays: true, relays: [] };
  const toggle = el('input', { type: 'checkbox', 'aria-label': 'Use public n0 relays' });
  const list = el('ul', { class: 'plain relay-list', 'aria-label': 'Custom relays' });
  const input = el('input', { placeholder: 'https://relay.example.org', 'aria-label': 'Relay URL' });
  const status = el('p', { class: 'muted', role: 'status' });
  const hint = el('p', { class: 'warning', role: 'status' });
  const saveBtn = el('button', { class: 'primary', text: 'Save network settings' });
  const table = el('dl', { class: 'net-table', 'aria-label': 'Network status' }, el('dd', { text: 'Loading…' }));

  const render = () => {
    const empty = !settings.use_public_relays && settings.relays.length === 0;
    hint.textContent = empty ? 'Add at least one relay, or turn public relays back on.' : '';
    saveBtn.disabled = empty;
    toggle.checked = settings.use_public_relays;
    list.replaceChildren(...settings.relays.map((url, i) => el('li', { class: 'relay-item' },
      el('span', { class: 'mono', text: url }),
      el('button', {
        class: 'icon-btn',
        'aria-label': `Remove ${url}`,
        title: 'Remove',
        icon: 'x',
        onclick: () => {
          settings.relays = settings.relays.filter((_, j) => j !== i);
          render();
        },
      }))));
    list.hidden = settings.relays.length === 0;
  };
  toggle.addEventListener('change', () => {
    settings.use_public_relays = toggle.checked;
    render();
  });
  const addBtn = el('button', {
    class: 'ghost',
    text: 'Add relay',
    onclick: () => {
      let url;
      try {
        url = new URL(input.value.trim());
      } catch {
        status.textContent = 'Enter a full URL, for example https://relay.example.org';
        return;
      }
      if (url.protocol !== 'https:') {
        status.textContent = 'A relay URL must use https.';
        return;
      }
      if (settings.relays.length >= 8) {
        status.textContent = 'Up to 8 custom relays.';
        return;
      }
      if (!settings.relays.includes(url.toString())) settings.relays = [...settings.relays, url.toString()];
      input.value = '';
      status.textContent = '';
      render();
    },
  });
  saveBtn.addEventListener('click', async () => {
    saveBtn.disabled = true;
    const saved = await actions.setNetworkSettings(settings);
    saveBtn.disabled = false;
    if (saved) {
      settings = saved;
      render();
    }
  });
  actions.loadNetworkSettings().then((loaded) => {
    if (loaded) {
      settings = loaded;
      render();
    }
  });
  render();

  const refresh = async () => {
    try {
      const s = await api.netStatus();
      table.replaceChildren(
        ...netRow('DHT size', s.dht ? `≈ ${Math.round(s.dht.size_estimate).toLocaleString('en-US')} nodes` : 'offline'),
        ...netRow('Confidence', s.dht ? `${Math.round(s.dht.confidence * 100)}%` : '—'),
        ...netRow('Reachability', s.dht ? (s.dht.firewalled ? 'firewalled' : 'reachable') : '—'),
        ...netRow('Peers', String(s.peers)),
        ...netRow('Relay', s.relay ?? 'none'),
      );
    } catch {
      table.replaceChildren(el('dd', { text: 'Status unavailable' }));
    }
  };
  refresh();
  netTimer = setInterval(refresh, NET_REFRESH_MS);

  return el('div', { class: 'settings-card' },
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Status' }),
      table),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Relays' }),
      el('label', { class: 'toggle-row' }, toggle, el('span', { text: 'Use public n0 relays' })),
      el('p', { class: 'muted', text: 'With public relays on, your relays are added to number0’s. With it off, only your relays are used, and you need at least one. Each must be an https URL.' }),
      list,
      el('div', { class: 'row' }, input, addBtn),
      status,
      hint,
      el('div', { class: 'row end' }, saveBtn)));
}

function netRow(label, value) {
  return [el('dt', { text: label }), el('dd', { text: value })];
}

// ---- Appearance ----

function appearance() {
  const current = getAppearance();
  const radio = (name, value, label, checked, onpick) => el('label', { class: 'radio-row' },
    el('input', { type: 'radio', name, value: String(value), checked, onchange: () => onpick(value) }),
    el('span', { text: label }));
  return el('div', { class: 'settings-card' },
    el('fieldset', { class: 'settings-block' },
      el('legend', { text: 'Message density' }),
      radio('density', 'cozy', 'Cozy', current.density === 'cozy', (v) => setAppearance({ density: v })),
      radio('density', 'compact', 'Compact', current.density === 'compact', (v) => setAppearance({ density: v }))),
    el('fieldset', { class: 'settings-block' },
      el('legend', { text: 'Font scale' }),
      radio('scale', 90, '90%', current.scale === 90, (v) => setAppearance({ scale: v })),
      radio('scale', 100, '100%', current.scale === 100, (v) => setAppearance({ scale: v })),
      radio('scale', 110, '110%', current.scale === 110, (v) => setAppearance({ scale: v }))));
}

// ---- Updates (version, the update check, and licences) ----

function updates() {
  const version = el('p', { text: 'Version …' });
  api.appVersion().then((v) => {
    version.textContent = `Dissent ${v}`;
  }).catch(() => {
    version.textContent = 'Dissent';
  });
  const link = (href, text) => el('a', { href, target: '_blank', rel: 'noopener noreferrer', text });
  const updateStatus = el('p', { class: 'muted', role: 'status' });
  const updateActions = el('div', { class: 'row' });
  const checkBtn = el('button', { class: 'ghost', icon: 'download', text: 'Check for updates' });
  checkBtn.addEventListener('click', async () => {
    checkBtn.disabled = true;
    updateStatus.textContent = 'Checking…';
    updateActions.replaceChildren();
    try {
      const info = await api.checkUpdate();
      if (info) {
        updateStatus.textContent = `Update available: v${info.version}`;
        updateActions.replaceChildren(el('button', { class: 'primary', text: 'View update', onclick: () => openUpdateDialog(info) }));
        actions.setUpdate(info);
      } else {
        updateStatus.textContent = 'You’re up to date.';
      }
    } catch {
      updateStatus.textContent = 'Couldn’t check for updates right now.';
    } finally {
      checkBtn.disabled = false;
    }
  });
  return el('div', { class: 'settings-card' },
    el('section', { class: 'settings-block' },
      el('img', { class: 'about-logo', src: 'app-icon-96.png', alt: 'Dissent logo', width: '64', height: '64' }),
      version,
      el('div', { class: 'row' }, checkBtn),
      updateStatus,
      updateActions,
      el('p', { class: 'muted', text: 'Decentralised chat. Your identity and messages stay on your devices and in the network, with no central server.' })),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Licenses' }),
      el('ul', { class: 'plain' },
        el('li', {}, 'Dissent is licensed under ', link('https://opensource.org/license/mit', 'MIT'), ' or ', link('https://www.apache.org/licenses/LICENSE-2.0', 'Apache-2.0'), '.'),
        el('li', {}, 'Icons by Lucide (ISC). ', link('https://lucide.dev/license', 'Lucide license'), '.'))));
}
