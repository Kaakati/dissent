// Transient messages in the bottom-right corner. Errors from a user action land here, as the old UI's ui.js's toast() did.
import { create } from 'zustand';

export interface Toast {
  id: number;
  text: string;
  kind: 'error' | 'info';
  action?: { label: string; run: () => void }; // a button on the toast, e.g. a reminder's Jump
}

interface ToastState {
  items: Toast[];
}

export const useToasts = create<ToastState>(() => ({ items: [] }));

const SHOW_FOR_MS = 4500;
let seq = 0;

export function toast(err: unknown, kind: Toast['kind'] = 'error', action?: Toast['action']): void {
  const text = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
  const id = ++seq;
  useToasts.setState((s) => ({ items: [...s.items, { id, text, kind, action }] }));
  setTimeout(() => {
    useToasts.setState((s) => ({ items: s.items.filter((t) => t.id !== id) }));
  }, SHOW_FOR_MS);
}
