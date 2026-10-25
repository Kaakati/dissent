// The Tauri globals this UI is built on. The desktop app injects them (tauri.conf.json sets withGlobalTauri). In a
// browser, src/mock/mock.js sets the same shapes before the app renders, so nothing below needs to know which it is.

export type Unlisten = () => void;

export interface TauriWindow {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  isMaximized?(): Promise<boolean>;
  isMinimized(): Promise<boolean>;
  isFocused(): Promise<boolean>;
}

export interface TauriGlobal {
  core: {
    invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
    convertFileSrc?(path: string): string; // the asset protocol: a local path the backend wrote, as an image source
  };
  event: {
    listen<T = unknown>(name: string, handler: (event: { payload: T }) => void): Promise<Unlisten>;
  };
  window?: {
    getCurrentWindow(): TauriWindow;
  };
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
    __DISSENT_MOCK__?: boolean;
  }
}

// The window the app is in, or null in a browser preview (where the window controls do nothing).
export function currentWindow(): TauriWindow | null {
  try {
    return window.__TAURI__?.window?.getCurrentWindow() ?? null;
  } catch {
    return null;
  }
}
