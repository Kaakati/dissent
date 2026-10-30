// A device started or stopped holding this person's offline mail (`inbox_host_changed`). The engine sends it once per
// change, signed by that device; it shows as a notice here and stays listed on Settings > Offline delivery.
import { toast } from './toasts';
import type { EngineEvent } from './types';

export function inboxNoticeText(name: string, on: boolean): string {
  return on
    ? `${name} now holds your messages while you are offline. Pick it in Settings › Offline delivery.`
    : `${name} stopped holding your messages while you are offline.`;
}

export function onInboxEvent(ev: EngineEvent): void {
  if (ev.kind === 'inbox_host_changed') toast(inboxNoticeText(ev.display_name, ev.on), 'info');
}
