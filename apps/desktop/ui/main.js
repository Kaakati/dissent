// Entry point. mock.js must stay first: it installs a stub backend when the page runs outside Tauri.
import { onEngineEvent } from './api.js';
import * as actions from './actions.js';
import { state } from './state.js';
import { toast } from './ui.js';
import { initRail } from './views/rail.js';
import { initSidebar } from './views/sidebar.js';
import { initChat } from './views/chat.js';
import { initRight } from './views/right.js';
import { initStatus } from './views/status.js';
import { initKeyboard } from './keyboard.js';
import { initUpdater } from './views/updater.js';
import { listenTo } from './api.js';
import { applyAppearance } from './appearance.js';

async function start() {
  applyAppearance();
  if (!window.__TAURI__) await import('./mock.js');
  initRail();
  initSidebar();
  initChat();
  initRight();
  initStatus();
  initKeyboard();
  initUpdater();
  listenTo('dissent://update-available', (info) => actions.setUpdate(info)).catch(() => {});
  listenTo('dissent://attachment-progress', (p) => actions.handleAttachmentProgress(p)).catch(() => {});
  if (window.__DISSENT_MOCK__) document.getElementById('mock-note').hidden = false;

  try {
    await onEngineEvent(actions.handleEngineEvent);
    await actions.boot();
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

start();
