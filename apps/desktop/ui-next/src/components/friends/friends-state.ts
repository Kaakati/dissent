// The Friends page's own state: the tab, the search and whether the Add Friend form is open. It lives outside the page so
// it stays between visits, as the current UI keeps it in module state (views/friends.js). Also the helpers the rows and
// the Active Now column share: presence, where a friend is in a call, and the conversation a row's menu is built for.
import { create } from 'zustand';
import { useShell } from '@/lib/store';
import type { DirectSummary, Person, Presence } from '@/lib/types';

export type FriendsTab = 'online' | 'all' | 'pending' | 'blocked';

interface FriendsView {
  tab: FriendsTab;
  query: string;
  addOpen: boolean;
  setTab: (tab: FriendsTab) => void;
  setQuery: (query: string) => void;
  setAddOpen: (addOpen: boolean) => void;
}

export const useFriendsView = create<FriendsView>()((set) => ({
  tab: 'online',
  query: '',
  addOpen: false,
  setTab: (tab) => set({ tab, addOpen: false }),
  setQuery: (query) => set({ query }),
  setAddOpen: (addOpen) => set({ addOpen }),
}));

// Presence of a person: their status when the engine gives one, else online or offline. Unknown counts as offline.
export function presenceOfPerson(p: Person): Presence {
  if (p.status) return p.status;
  return p.online ? 'online' : 'offline';
}

export const isUp = (p: Person): boolean => presenceOfPerson(p) !== 'offline';

// Where a friend is in a call right now, as the line under their name: a group or direct call, or a voice channel of the
// open server. Null when they are not in one.
export function callOf(rootId: string): string | null {
  const s = useShell.getState();
  for (const conv of s.conversations) {
    const live = conv.live_call;
    if (!live) continue;
    const inCall = live.participants?.some((m) => m.root_id === rootId) || (live.scope === 'dm' && conv.kind === 'dm' && conv.peer === rootId);
    if (inCall) return conv.kind === 'group' ? conv.name : 'a direct call';
  }
  const community = s.community;
  if (community) {
    for (const voice of community.voice_channels ?? []) {
      if (!voice.participants.some((m) => m.root_id === rootId)) continue;
      const channel = community.channels.find((c) => c.id === voice.id);
      return `#${channel?.name ?? ''} · ${community.name}`;
    }
  }
  return null;
}

// The conversation shape the row menu expects, for someone who has no conversation row yet.
export const directOf = (p: Person): DirectSummary => ({ kind: 'dm', peer: p.root_id, display_name: p.display_name, pinned: false });
