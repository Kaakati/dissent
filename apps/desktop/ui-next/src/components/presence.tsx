import { cn } from '@/lib/utils';
import type { Presence as PresenceState } from '@/lib/types';

export const PRESENCE_LABEL: Record<PresenceState, string> = {
  online: 'Online',
  idle: 'Idle',
  dnd: 'Do Not Disturb',
  offline: 'Offline',
};

// The glyph for a presence state. Its shape comes from shell.css, so colour is never the only cue.
export function PresenceDot({ status, className }: { status: PresenceState; className?: string }) {
  return <span role="img" aria-label={PRESENCE_LABEL[status]} className={cn('presence', status, className)} />;
}
