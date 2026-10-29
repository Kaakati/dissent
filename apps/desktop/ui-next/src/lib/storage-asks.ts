// Seeders asking this owner or admin device for a storage grant, per server (storage_grant_asks). The engine tells the
// UI once per new ask (`storage_grant_asked`), which shows a notification and refreshes the count. Server Settings ›
// Members and the servers' Server Settings entries show the count as a badge.
import { create } from 'zustand';
import { api } from './api';
import { toast } from './toasts';
import type { EngineEvent, Id } from './types';

export const useStorageAsks = create<{ asks: Record<Id, Id[]> }>(() => ({ asks: {} }));

// Reads the asks of one server again. A device that cannot grant holds none, and an error keeps the last answer.
export async function loadStorageAsks(communityId: Id): Promise<Id[]> {
  try {
    const asks = (await api.storageGrantAsks(communityId)) as Id[];
    useStorageAsks.setState((s) => ({ asks: { ...s.asks, [communityId]: asks } }));
    return asks;
  } catch {
    return useStorageAsks.getState().asks[communityId] ?? [];
  }
}

// The badge text for a count of asks: "1 seeder asks for a grant", or nothing.
export function asksLabel(count: number): string {
  if (!count) return '';
  return count === 1 ? '1 seeder asks for a grant' : `${count} seeders ask for a grant`;
}

export function onStorageAskEvent(ev: EngineEvent): void {
  if (ev.kind === 'storage_grant_asked') {
    toast(`${ev.display_name} asks for a storage grant in ${ev.community_name}. Open Server Settings › Members to grant it.`, 'info');
    void loadStorageAsks(ev.community_id);
  } else if (ev.kind === 'community_updated' && ev.community_id in useStorageAsks.getState().asks) {
    void loadStorageAsks(ev.community_id);
  }
}
