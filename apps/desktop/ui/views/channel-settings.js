// Channel settings: a full-page overlay built like Settings. Overview holds the name, slowmode and content
// visibility, and voice channels add bitrate, user limit and relay. Permissions is not built. Invites lists the
// server's invites with Revoke. Changes wait in a save bar until Save. Owner and admin only.
import { state, canManage } from '../state.js';
import * as actions from '../actions.js';
import { api } from '../api.js';
import { el, icon, toast } from '../ui.js';
import { confirmDialog, openInviteDialog, cleanChannelName } from './dialogs.js';

const SLOWMODE = [
  ['Off', 0], ['5s', 5], ['10s', 10], ['15s', 15], ['30s', 30], ['1m', 60], ['2m', 120],
  ['5m', 300], ['10m', 600], ['15m', 900], ['30m', 1800], ['1h', 3600], ['2h', 7200], ['6h', 21600],
];
const VISIBILITY = [
  ['default', 'Default', 'Messages in this channel are visible to everyone who can view it.'],
  ['spoiler', 'Spoiler Channel', 'Messages and attachments are blurred until someone clicks to reveal them.'],
  ['age', 'Age-Restricted Channel', 'People must confirm they are of age once before they can view this channel.'],
];
const BITRATE_HELP = 'Going above 64 kbps may adversely affect people on poor connections.';
const RELAY_HELP = 'Picks which relay helps people who can’t connect directly to each other in this channel.';
const SECTIONS = [
  { key: 'overview', label: 'Overview', icon: null },
  { key: 'permissions', label: 'Permissions', icon: null },
  { key: 'invites', label: 'Invites', icon: null },
];

let page = null;
let opener = null;
let section = 'overview';
// The channel as the engine last reported it, and the edits not yet saved.
let ctx = null;

const fromChannel = (ch) => ({
  name: ch.name,
  slowmode_s: ch.slowmode_s ?? 0,
  visibility: ch.visibility ?? 'default',
  bitrate_kbps: ch.bitrate_kbps ?? 64,
  user_limit: ch.user_limit ?? 0,
  relay: ch.relay || 'automatic',
});

// The fields that differ between the saved channel and the edits.
const changes = () => Object.fromEntries(Object.entries(ctx.draft).filter(([key, value]) => ctx.saved[key] !== value));
const dirty = () => Object.keys(changes()).length > 0;

export function openChannelSettings(communityId, channelId) {
  const community = state.community;
  const ch = community?.id === communityId ? community.channels.find((c) => c.id === channelId) : null;
  if (!ch || !canManage()) return;
  if (page) closeChannelSettings(true);
  const saved = fromChannel(ch);
  ctx = { communityId, channelId, voice: Boolean(ch.voice), saved, draft: { ...saved } };
  opener = document.activeElement;
  page = el('div', { class: 'settings-page channel-settings', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Channel settings' },
    el('div', { class: 'settings-side' }, el('nav', { class: 'settings-nav', 'aria-label': 'Channel settings sections' })),
    el('div', { class: 'settings-content' }, el('div', { class: 'channel-settings-main' })),
    el('div', { class: 'settings-close-col' },
      el('button', { class: 'settings-close icon-btn', 'aria-label': 'Close channel settings', title: 'Close (Esc)', icon: 'x', onclick: () => closeChannelSettings() }),
      el('span', { class: 'esc-label', text: 'ESC' })));
  document.body.append(page);
  document.addEventListener('keydown', onKey, true);
  show('overview');
  page.querySelector('.settings-nav button[aria-current="true"]')?.focus();
}

// Closes the overlay. With unsaved edits, asks first; `force` skips the question when a new channel is opened.
export async function closeChannelSettings(force = false) {
  if (!page) return;
  if (!force && dirty() && !(await confirmDialog('Discard your unsaved channel changes?', 'Discard'))) return;
  document.removeEventListener('keydown', onKey, true);
  page.remove();
  page = null;
  ctx = null;
  if (opener?.isConnected) opener.focus();
  opener = null;
}

function onKey(e) {
  if (e.key === 'Escape' && !document.querySelector('dialog[open]')) {
    e.preventDefault();
    e.stopPropagation();
    closeChannelSettings();
  }
}

function renderNav() {
  const nav = page.querySelector('.settings-nav');
  nav.replaceChildren(
    el('h2', { class: 'channel-settings-title' }, icon(ctx.voice ? 'volume-2' : 'hash'), el('span', { text: `${ctx.saved.name} · ${ctx.voice ? 'Voice' : 'Text'} Channels` })),
    ...SECTIONS.map((s) => el('button', {
      class: 'settings-nav-item',
      text: s.label,
      'aria-current': s.key === section ? 'true' : null,
      onclick: () => show(s.key),
    })),
    el('div', { class: 'sidebar-sep', role: 'separator' }),
    el('button', { class: 'settings-nav-item danger', icon: 'trash-2', text: 'Delete Channel', onclick: () => deleteThis() }));
}

function show(which) {
  section = which;
  renderNav();
  const titles = { overview: 'Overview', permissions: 'Permissions', invites: 'Invites' };
  const body = { overview, permissions, invites: invitesSection }[section]();
  page.querySelector('.settings-content').replaceChildren(el('div', { class: 'channel-settings-main' },
    el('header', { class: 'settings-head' }, el('h1', { text: titles[section] })),
    body,
    saveBar()));
}

// ---- Overview ----

function field(label, control, help) {
  return el('section', { class: 'settings-block' },
    el('h2', { text: label }),
    control,
    help ? el('p', { class: 'muted', text: help }) : null);
}

function overview() {
  const d = ctx.draft;
  const nameInput = el('input', {
    value: d.name,
    maxLength: 64,
    'aria-label': 'Channel Name',
    oninput: (e) => {
      const next = cleanChannelName(e.target.value, ctx.voice);
      e.target.value = next;
      d.name = next;
      refresh();
    },
  });
  const slowmode = el('select', {
    'aria-label': 'Slowmode',
    onchange: (e) => { d.slowmode_s = Number(e.target.value); refresh(); },
  }, SLOWMODE.map(([label, seconds]) => el('option', { value: String(seconds), selected: seconds === d.slowmode_s, text: label })));
  const visibility = el('fieldset', { class: 'settings-block' },
    el('legend', { text: 'Content Visibility' }),
    VISIBILITY.map(([value, label, help]) => el('label', { class: 'kind-row' },
      el('input', { type: 'radio', name: 'channel-visibility', value, checked: d.visibility === value, onchange: () => { d.visibility = value; refresh(); } }),
      el('span', { class: 'kind-body' }, el('span', { class: 'kind-title', text: label }), el('span', { class: 'kind-desc', text: help })))));
  const nodes = [field('Channel Name', nameInput)];
  if (!ctx.voice) nodes.push(field('Slowmode', slowmode, 'Members wait this long between messages. Owners and admins are not affected.'));
  nodes.push(visibility);
  if (ctx.voice) {
    const bitrateValue = el('output', { text: `${d.bitrate_kbps} kbps` });
    const bitrate = el('input', {
      type: 'range', min: 8, max: 96, step: 1, value: d.bitrate_kbps, 'aria-label': 'Bitrate',
      oninput: (e) => { d.bitrate_kbps = Number(e.target.value); bitrateValue.textContent = `${d.bitrate_kbps} kbps`; refresh(); },
    });
    const limitValue = el('output', { text: d.user_limit === 0 ? '∞' : String(d.user_limit) });
    const limit = el('input', {
      type: 'range', min: 0, max: 99, step: 1, value: d.user_limit, 'aria-label': 'User Limit',
      oninput: (e) => { d.user_limit = Number(e.target.value); limitValue.textContent = d.user_limit === 0 ? '∞' : String(d.user_limit); refresh(); },
    });
    // The community names its relays as URLs. Automatic is sent as 'automatic', which the desktop maps to none.
    const relays = [{ id: 'automatic', name: 'Automatic' }, ...(state.community?.relays ?? []).map((url) => ({ id: url, name: new URL(url).host }))];
    const relay = el('select', {
      'aria-label': 'Relay',
      onchange: (e) => { d.relay = e.target.value; refresh(); },
    }, relays.map((r) => el('option', { value: r.id, selected: r.id === d.relay, text: r.name })));
    nodes.push(
      field('Bitrate', el('div', { class: 'slider-row' }, bitrate, bitrateValue), BITRATE_HELP),
      field('User Limit', el('div', { class: 'slider-row' }, limit, limitValue), 'Most people who can join at once. ∞ means no limit.'),
      field('Relay', relay, RELAY_HELP));
  }
  return el('div', { class: 'settings-body-stack' }, nodes);
}

function permissions() {
  return el('section', { class: 'settings-block' },
    el('h2', { text: 'Coming soon' }),
    el('p', { class: 'muted', text: 'Per-channel roles are not built yet, so channel permissions cannot be set here.' }));
}

// ---- Invites ----

function invitesSection() {
  const list = el('div', { class: 'invite-list', role: 'list', 'aria-label': 'Active invites' }, el('p', { class: 'muted', text: 'Loading invites…' }));
  const create = el('button', { class: 'primary', icon: 'user-plus', text: 'Create invite', onclick: () => openInviteDialog(ctx.communityId) });
  loadInvites(list);
  return el('div', { class: 'settings-body-stack' },
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Active invites' }),
      el('p', { class: 'muted', text: 'Revoking a link stops new joins with it. Members already in the server stay. If a link has reached someone who must not read this server, ban them or rotate the server’s keys.' }),
      list),
    el('div', { class: 'row' }, create));
}

async function loadInvites(list) {
  let items;
  try {
    items = await api.listInvites(ctx.communityId);
  } catch (err) {
    list.replaceChildren(el('p', { class: 'muted', text: `Invites are not available: ${err}` }));
    return;
  }
  if (!items.length) {
    list.replaceChildren(el('p', { class: 'muted', text: 'No active invites.' }));
    return;
  }
  list.replaceChildren(...items.map((inv) => el('div', { class: 'invite-row', role: 'listitem' },
    el('code', { class: 'invite-link', text: inv.link, title: inv.link }),
    el('button', {
      class: 'ghost',
      text: 'Revoke',
      'aria-label': `Revoke invite ${inv.link}`,
      onclick: async () => {
        try {
          await api.revokeInvite(ctx.communityId, inv.id);
          loadInvites(list);
        } catch (err) {
          toast(err);
        }
      },
    }))));
}

// ---- Save bar and delete ----

function saveBar() {
  const bar = el('div', { class: 'unsaved-bar', role: 'region', 'aria-label': 'Unsaved changes', hidden: !dirty() },
    el('span', { class: 'unsaved-text', text: 'Careful — you have unsaved changes!' }),
    el('button', { class: 'ghost', text: 'Reset', onclick: () => { ctx.draft = { ...ctx.saved }; show(section); } }),
    el('button', { class: 'primary', text: 'Save', onclick: save }));
  bar.dataset.bar = 'true';
  return bar;
}

// Keeps the save bar in step with the edits without rebuilding the form, so focus stays where it was.
function refresh() {
  const bar = page?.querySelector('[data-bar]');
  if (bar) bar.hidden = !dirty();
}

async function save() {
  const patch = changes();
  if (!Object.keys(patch).length) return;
  const ok = await actions.updateChannel(ctx.communityId, ctx.channelId, patch);
  if (!ok || !page) return;
  const ch = state.community?.channels.find((c) => c.id === ctx.channelId);
  if (ch) ctx.saved = fromChannel(ch);
  ctx.draft = { ...ctx.saved };
  show(section);
  toast('Channel saved', 'info');
}

async function deleteThis() {
  const name = ctx.saved.name;
  const ok = await confirmDialog(`Are you sure you want to delete ${ctx.voice ? '' : '#'}${name}? Its messages will be lost for everyone.`, 'Delete Channel');
  if (!ok) return;
  const done = await actions.deleteChannel(ctx.communityId, ctx.channelId);
  if (done) closeChannelSettings(true);
}
