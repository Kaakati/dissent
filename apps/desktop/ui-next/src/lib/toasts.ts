// Transient messages in the bottom-right corner. Errors from a user action land here, as ui/ui.js's toast() did.
import { create } from 'zustand';

export interface Toast {
  id: number;
  text: string;
  kind: 'error' | 'info';
}

interface ToastState {
  items: Toast[];
}

export const useToasts = create<ToastState>(() => ({ items: [] }));

const SHOW_FOR_MS = 4500;
let seq = 0;

export function toast(err: unknown, kind: Toast['kind'] = 'error'): void {
  const text = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
  const id = ++seq;
  useToasts.setState((s) => ({ items: [...s.items, { id, text, kind }] }));
  setTimeout(() => {
    useToasts.setState((s) => ({ items: s.items.filter((t) => t.id !== id) }));
  }, SHOW_FOR_MS);
}
