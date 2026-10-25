// What the chat pane has open, as one chat target, or null when no conversation is on screen (Friends, Transfers, a voice
// channel's board, nothing). Shared by the chat store, the alerts and the header, so they agree on one key.
import type { ShellState } from './store';
import { channelKey } from './store';
import type { ChatTarget } from './types';

export function openedTarget(s: Pick<ShellState, 'page' | 'route' | 'community' | 'channelId' | 'voiceChannelId' | 'peer' | 'group'>): ChatTarget | null {
  if (s.page) return null;
  if (s.route.kind === 'server') {
    if (!s.community || !s.channelId || s.voiceChannelId) return null;
    return { kind: 'channel', communityId: s.community.id, channelId: s.channelId, key: channelKey(s.community.id, s.channelId) };
  }
  if (!s.peer) return null;
  if (s.group) return { kind: 'group', groupId: s.peer, key: `group:${s.peer}` };
  return { kind: 'dm', peer: s.peer, key: `dm:${s.peer}` };
}
