// Whether the user is watching a conversation right now: it is open, the window is on screen with focus, and the
// message list is at its end. A message that arrives there is seen as it lands, so it sounds no alert and raises no
// desktop notification (which on Windows also leaves a badge on the taskbar until the Action Center is opened).
import { useShell } from './store';
import { openedTarget } from './target';

let atLatest = true;

// The message list reports whether it is scrolled to its end. A conversation that is switched to starts there.
export function setAtLatest(at: boolean): void {
  atLatest = at;
}

// The page has focus and is not hidden. On Windows the native window reports no focus while the webview inside it has
// it, so the page's own focus is the one to trust.
export function pageFocused(): boolean {
  return document.visibilityState !== 'hidden' && document.hasFocus();
}

export function viewing(key: string): boolean {
  return openedTarget(useShell.getState())?.key === key && atLatest && pageFocused();
}
