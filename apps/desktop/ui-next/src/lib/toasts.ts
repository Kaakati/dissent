// Transient messages in the bottom-right corner. Errors from a user action land here, as the old UI's ui.js's toast() did.
import { create } from 'zustand';

export interface Toast {
  id: number;
  text: string;
  kind: 'error' | 'info';
  actions?: ToastAction[]; // buttons on the toast, e.g. a reminder's Jump
}

export interface ToastAction {
  label: string;
  run: () => void;
}

interface ToastState {
  items: Toast[];
}

export const useToasts = create<ToastState>(() => ({ items: [] }));

const SHOW_FOR_MS = 4500;
let seq = 0;

// `action` is one button, or several with how long the toast stays, for one that offers Undo.
export function toast(err: unknown, kind: Toast['kind'] = 'error', action?: ToastAction | { actions: ToastAction[]; ms?: number }): void {
  const text = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
  const id = ++seq;
  const actions = !action ? undefined : 'actions' in action ? action.actions : [action];
  const ms = (action && 'ms' in action ? action.ms : undefined) ?? SHOW_FOR_MS;
  useToasts.setState((s) => ({ items: [...s.items, { id, text, kind, actions }] }));
  setTimeout(() => {
    useToasts.setState((s) => ({ items: s.items.filter((t) => t.id !== id) }));
  }, ms);
}
