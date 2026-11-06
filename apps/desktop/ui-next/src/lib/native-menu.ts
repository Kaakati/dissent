// The webview's own right-click menu (Back, Reload, Print, Inspect) is not for an app, so one listener at the root turns
// it off. Editable fields keep it, for cut, paste and spelling suggestions. Our menus call preventDefault in their own
// handlers, which run first, so they are left alone. Selected text gets a small menu of ours with Copy instead.
import { Copy } from 'lucide-react';
import { copyText } from './clipboard';
import { openContextMenu } from './menu';
import { toast } from './toasts';

const isEditable = (node: EventTarget | null): boolean =>
  node instanceof HTMLElement && (node.isContentEditable || node.closest('input, textarea, [contenteditable]:not([contenteditable="false"])') !== null);

// The selected text when a right-click lands on it, outside an editable field. A row with a menu of its own (a message)
// leaves the click to the Copy menu then, so the selection can still be copied.
export function selectionAt(target: EventTarget | null): string {
  const selection = window.getSelection();
  const text = selection?.toString() ?? '';
  if (!selection || !text.trim() || !(target instanceof Node) || isEditable(target)) return '';
  return selection.containsNode(target, true) ? text : '';
}

let installed = false;

export function installContextMenuGuard(): void {
  if (installed) return;
  installed = true;
  document.addEventListener('contextmenu', (e) => {
    if (e.defaultPrevented || isEditable(e.target)) return;
    e.preventDefault();
    const text = window.getSelection()?.toString() ?? '';
    if (!text.trim()) return;
    openContextMenu(e.clientX, e.clientY, [
      {
        label: 'Copy',
        icon: Copy,
        onPick: () => void copyText(text).then((ok) => toast(ok ? 'Copied' : 'Could not copy it', ok ? 'info' : 'error')),
      },
    ]);
  });
}
