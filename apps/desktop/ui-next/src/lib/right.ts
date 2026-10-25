// The right column's open state. The server member list is open until the user closes it, as the current UI has it.
// The DM profile panel's open state is kept on this device; storage can be unavailable, so it then starts open.
import { create } from 'zustand';
import type { Id } from './types';

export const PROFILE_PANEL_KEY = 'dissent.profilePanel.v1';
const savedProfileOpen = (): boolean => {
  try {
    return window.localStorage.getItem(PROFILE_PANEL_KEY) !== 'closed';
  } catch {
    return true;
  }
};

interface RightState {
  membersOpen: boolean;
  profileOpen: boolean; // the DM profile panel
  fileDetail: Id | null; // the attachment whose transfer details are open (replaces the other panels)
}

export const useRight = create<RightState>(() => ({ membersOpen: true, profileOpen: savedProfileOpen(), fileDetail: null }));

export function toggleMembers(): void {
  useRight.setState((s) => ({ membersOpen: !s.membersOpen }));
}

export function toggleProfile(): void {
  const profileOpen = !useRight.getState().profileOpen;
  useRight.setState({ profileOpen });
  try {
    window.localStorage.setItem(PROFILE_PANEL_KEY, profileOpen ? 'open' : 'closed');
  } catch {
    // Storage unavailable: the choice lasts this session only.
  }
}

// The file whose details panel is open in the right column; null closes it.
export function openFileDetail(attachmentId: Id | null): void {
  useRight.setState({ fileDetail: attachmentId });
}
