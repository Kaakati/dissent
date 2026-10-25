// The dialog stack and confirm(), the React counterpart of ui/ui.js openDialog and views/dialogs.js confirmDialog.
// Open a dialog with openDialog({ title, content }); `content` is given the close function, so a form can close itself
// after a successful action. confirm() resolves true on the action and false on Cancel, Escape or the scrim. The
// DialogHost component (components/dialogs/dialog-host.tsx) renders whatever is open here.
import type { ReactNode } from 'react';
import { create } from 'zustand';

export interface DialogOptions {
  title: string;
  sub?: string | null;
  className?: string;
  wide?: boolean;
  content: (close: () => void) => ReactNode;
  footer?: (close: () => void) => ReactNode;
  onClose?: () => void;
}

export interface DialogEntry extends DialogOptions {
  id: number;
  opener: HTMLElement | null; // the element that had the focus when it opened, which gets it back on close
}

export interface ConfirmOptions {
  message: string;
  okLabel: string;
  title?: string;
  danger?: boolean;
}

export interface ConfirmEntry extends ConfirmOptions {
  id: number;
  opener: HTMLElement | null;
  resolve: (ok: boolean) => void;
}

interface DialogState {
  dialogs: DialogEntry[];
  confirm: ConfirmEntry | null;
}

export const useDialogs = create<DialogState>(() => ({ dialogs: [], confirm: null }));

let seq = 0;

// Removes a dialog from the stack once. onClose runs once, however the dialog was closed.
export function closeDialogById(id: number): void {
  const entry = useDialogs.getState().dialogs.find((d) => d.id === id);
  if (!entry) return;
  useDialogs.setState((s) => ({ dialogs: s.dialogs.filter((d) => d.id !== id) }));
  entry.onClose?.();
}

// The element with the focus now, so the dialog can give the focus back when it closes.
const focusedNow = (): HTMLElement | null => (document.activeElement instanceof HTMLElement ? document.activeElement : null);

export function openDialog(options: DialogOptions): { close: () => void } {
  const id = ++seq;
  useDialogs.setState((s) => ({ dialogs: [...s.dialogs, { ...options, id, opener: focusedNow() }] }));
  return { close: () => closeDialogById(id) };
}

// Only one confirmation is open at a time. A new one answers the old one with false.
export function confirm(options: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    const id = ++seq;
    useDialogs.getState().confirm?.resolve(false);
    useDialogs.setState({ confirm: { ...options, id, opener: focusedNow(), resolve } });
  });
}

// Called by the host when the confirmation is answered, by either button, Escape or the scrim.
export function answerConfirm(id: number, ok: boolean): void {
  const current = useDialogs.getState().confirm;
  if (!current || current.id !== id) return;
  useDialogs.setState({ confirm: null });
  current.resolve(ok);
}
