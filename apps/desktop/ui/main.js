// Entry point. mock.js must stay first: it installs a stub backend when the page runs outside Tauri.
import { onEngineEvent } from './api.js';
import * as actions from './actions.js';
import { state } from './state.js';
import { toast } from './ui.js';
import { initRail } from './views/rail.js';
import { initSidebar } from './views/sidebar.js';
import { initChat } from './views/chat.js';
import { initMembers } from './views/members.js';
import { initStatus } from './views/status.js';

async function start() {
  if (!window.__TAURI__) await import('./mock.js');
  initRail();
  initSidebar();
  initChat();
  initMembers();
  initStatus();
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
