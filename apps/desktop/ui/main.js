// Entry point. mock.js must stay first: it installs a stub backend when the page runs outside Tauri.
import { onEngineEvent, api } from './api.js';
import { loadCapabilities } from './features.js';
import { initTitlebar } from './views/titlebar.js';
import * as actions from './actions.js';
import { state } from './state.js';
import { toast } from './ui.js';
import { applyTyping } from './typing.js';
import { initRail } from './views/rail.js';
import { initSidebar } from './views/sidebar.js';
import { initChat } from './views/chat.js';
import { initRight } from './views/right.js';
import { showIncomingCall, dismissIncomingCall } from './views/incoming.js';
import { initStatus } from './views/status.js';
import { initKeyboard } from './keyboard.js';
import { initUpdater } from './views/updater.js';
import { listenTo } from './api.js';
import { applyAppearance } from './appearance.js';
import { has } from './features.js';
import { showOnboarding } from './views/onboarding.js';
import { openSettingsPage } from './views/settings.js';

// Input counts as activity for auto-idle. The engine ignores repeats, so the UI reports at most once every 30 seconds.
const ACTIVITY_EVERY_MS = 30_000;
function watchActivity() {
  if (!has('status')) return;
  let last = 0;
  const report = () => {
    const now = Date.now();
    if (now - last < ACTIVITY_EVERY_MS) return;
    last = now;
    api.recordActivity().catch(() => {});
  };
  for (const type of ['pointerdown', 'keydown', 'wheel']) window.addEventListener(type, report, { capture: true, passive: true });
}

async function start() {
  applyAppearance();
  if (!window.__TAURI__) await import('./mock.js');
  await loadCapabilities(api);
  initTitlebar();
  initRail();
  initSidebar();
  initChat();
  initRight();
  initStatus();
  initKeyboard();
  initUpdater();
  listenTo('dissent://update-available', (info) => actions.setUpdate(info)).catch(() => {});
  listenTo('dissent://attachment-progress', (p) => actions.handleAttachmentProgress(p)).catch(() => {});
  listenTo('dissent://transfers', (p) => actions.handleTransfersEvent(p)).catch(() => {});
  listenTo('dissent://call-state', (c) => actions.handleCallEvent(c)).catch(() => {});
  listenTo('dissent://status-changed', (s) => actions.applyStatus(s)).catch(() => {});
  listenTo('dissent://typing-changed', (p) => applyTyping(p)).catch(() => {});
  listenTo('dissent://call-ringing', (p) => { if (has('calls')) showIncomingCall(p); }).catch(() => {});
  listenTo('dissent://notification-click', (p) => actions.openNotifiedConversation(p?.conv)).catch(() => {});
  if (window.__DISSENT_MOCK__) document.getElementById('mock-note').hidden = false;

  try {
    await onEngineEvent(actions.handleEngineEvent);
    // A call that rang out, or was answered on another device, is no longer ringing here.
    onEngineEvent((ev) => {
      if (ev?.kind === 'call_missed' || ev?.kind === 'call_answered_elsewhere') dismissIncomingCall(ev.call_id);
    }).catch(() => {});
    await actions.boot();
    await actions.loadStatus();
    watchActivity();
    if (has('onboarding') && state.me?.first_run) showOnboarding();
    if (window.__DISSENT_MOCK__) await openMockView(new URLSearchParams(location.search));
  } catch (err) {
    toast(err);
  }
  try {
    await actions.loadVersion();
  } catch {
    state.version = '';
  }
  document.getElementById('version').textContent = state.version ? `v${state.version}` : '';
}

// Mock-only: ?view=call | dm&peer=maya | contacts | transfers | onboarding | group opens that state directly.
async function openMockView(params) {
  const view = params.get('view');
  if (!view) return;
  if (view === 'call') {
    await actions.openServer('c1');
    await actions.openVoiceChannel('vc1');
  } else if (view === 'dm') {
    const peer = [...state.conversations].find((c) => c.kind !== 'group' && c.display_name?.toLowerCase() === String(params.get('peer') ?? 'maya'))?.peer;
    if (peer) await actions.openDm(peer, state.conversations.find((c) => c.peer === peer).display_name);
    // ?file=<name> opens that attachment's transfer panel (FilesDirect board).
    const fileName = params.get('file');
    const file = fileName ? [...state.dmMessages, ...state.messages].flatMap((m) => m.attachments ?? []).find((a) => a.name === fileName) : null;
    if (file) actions.openFileDetail(file.id);
  } else if (view === 'files') {
    // FilesServer board: #hardware with the transfer panel open on the walkthrough video.
    await actions.openServer('c1', 'ch2');
    const file = state.messages.flatMap((m) => m.attachments ?? []).find((a) => a.name === 'workshop-walkthrough.mp4');
    if (file) actions.openFileDetail(file.id);
  } else if (view === 'incoming' || view === 'joincall') {
    // Call boards: the Saturday group is open behind the modal (incoming) or in the lobby (joincall).
    const conv = state.conversations.find((c) => c.kind === 'group' && c.name === 'Saturday workshop crew');
    if (conv) await actions.openGroup(conv);
    if (view === 'incoming') window.__mockRing?.(params.get('scope') ?? 'group');
  } else if (view === 'contacts' || view === 'transfers') {
    await actions.openPage(view);
  } else if (view === 'onboarding') {
    showOnboarding();
  } else if (view === 'settings') {
    openSettingsPage(params.get('section') ?? 'account');
  } else if (view === 'group') {
    const conv = state.conversations.find((c) => c.kind === 'group');
    if (conv) await actions.openGroup(conv);
  }
}

start();
