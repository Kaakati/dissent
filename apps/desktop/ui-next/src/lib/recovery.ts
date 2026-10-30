// Guardian recovery on this device: the recovery in progress, read from the engine and re-read when a guardian's answer
// arrives (`recovery_changed`). The dialogs in components/recovery read it. See dissent-core recovery_flow.rs.
import { create } from 'zustand';
import { api } from './api';
import type { EngineEvent, RecoveryView } from './types';

interface RecoveryState {
  view: RecoveryView | null;
  loaded: boolean;
  set: (view: RecoveryView | null) => void;
}

export const useRecovery = create<RecoveryState>()((set) => ({
  view: null,
  loaded: false,
  set: (view) => set({ view, loaded: true }),
}));

// Reads the recovery in progress again. An error keeps what was shown.
export async function loadRecovery(): Promise<RecoveryView | null> {
  try {
    const view = await api.recoveryStatus();
    useRecovery.getState().set(view ?? null);
    return view ?? null;
  } catch {
    return useRecovery.getState().view;
  }
}

export function onRecoveryEvent(ev: EngineEvent): void {
  if (ev.kind === 'recovery_changed') void loadRecovery();
}

// Only the digits of a code someone typed, so "123 456 789", "123-456-789" and "123456789" are the same.
export const codeDigits = (code: string): string => code.replace(/\D/g, '');

// "2 of 3 guardians approved", or the count alone while the guardian list has not arrived.
export function progressText(view: RecoveryView): string {
  if (view.threshold === null) {
    const n = view.approvals;
    return `${n} approval${n === 1 ? '' : 's'} so far. The guardian list arrives with the first approval.`;
  }
  return `${Math.min(view.approvals, view.threshold)} of ${view.threshold} guardian approvals needed`;
}
