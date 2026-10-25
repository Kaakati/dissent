// The browser preview runs against the current UI's mock backend, so both UIs share one mock. Inside the desktop app
// window.__TAURI__ already exists and this does nothing. The import path is literal so Vite bundles the mock.
export async function loadMockBackend(): Promise<void> {
  if (!window.__TAURI__) await import('../mock/mock.js');
}
