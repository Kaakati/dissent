// A counter the profile views read, so they reload user_profile when something about a person changed (a friend
// request, a safety number compared, a nickname). The views hold no copy of the store; they only re-read.
import { create } from 'zustand';

export const useProfileVersion = create<{ version: number }>()(() => ({ version: 0 }));

export function refreshProfiles(): void {
  useProfileVersion.setState((s) => ({ version: s.version + 1 }));
}
