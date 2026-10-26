// Entry point. In the desktop app the Tauri globals exist already. In a browser the current UI's mock backend is
// loaded first, so the shell runs against the same fixtures as apps/desktop/ui.
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
import './shell.css';
import App from './App';
import { runGate } from './components/startup/gate';
import { api } from './lib/api';
import { applyAppearance } from './lib/appearance';
import { setCapabilities } from './lib/features';
import { startShellEvents } from './lib/events';
import { loadMockBackend } from './lib/mock';
import { useShell, type SettingsSection } from './lib/store';
import { toast } from './lib/toasts';

// Mock-only: ?view=onboarding | settings&section=<name> opens that state directly. Other batches add their views here.
async function openMockView(params: URLSearchParams): Promise<void> {
  const view = params.get('view');
  if (view === 'onboarding') {
    useShell.setState({ onboarding: true, titleOverride: 'Welcome to Dissent' });
  } else if (view === 'settings') {
    useShell.getState().openSettings((params.get('section') ?? 'account') as SettingsSection);
  }
}

async function start(): Promise<void> {
  applyAppearance();
  await loadMockBackend();
  // A portable profile waits here for its passphrase. Nothing below may call the engine before that.
  const vault = await api.vaultStatus();
  if (vault.chooser || vault.locked) await runGate(vault);
  // Capabilities are read once at boot. A backend without the command supports nothing new.
  setCapabilities(await api.capabilities().catch(() => null));

  const root = document.getElementById('root');
  if (!root) throw new Error('index.html has no #root element');
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );

  // Listeners go up before the first load, as the old UI's main.js does, so no engine event between the two is missed.
  startShellEvents();
  const shell = useShell.getState();
  try {
    await shell.boot();
    await shell.loadStatus();
    shell.startFirstRun();
    if (window.__DISSENT_MOCK__) await openMockView(new URLSearchParams(location.search));
  } catch (err) {
    toast(err);
  }
  try {
    useShell.setState({ version: await api.appVersion() });
  } catch {
    // The version stays blank: the window still works.
  }
}

void start();
