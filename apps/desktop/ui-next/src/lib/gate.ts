// Switches older apps cannot follow: custom roles' permissions, member-only details and the history setting. An offline
// member never holds one back, and an online device on an old app does until the owner overrides it. Either way the
// owner is told by name who is affected and what they will see, and the engine goes ahead only for the devices listed
// here (see `GateStatus` in dissent-core).
import { confirm } from '@/lib/dialog';
import type { GateDevice, GateStatus } from '@/lib/types';

type GateLists = Pick<GateStatus, 'offline' | 'outdated'>;

// "Maya, Omar and 3 others", each person once however many devices they have.
export function gateNames(devices: GateDevice[], limit = 5): string {
  const names = [...new Set(devices.map((d) => d.name || 'Someone'))];
  if (names.length <= 1) return names[0] ?? '';
  if (names.length <= limit) return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
  return `${names.slice(0, limit).join(', ')} and ${names.length - limit} others`;
}

const devicesText = (n: number) => `${n} member ${n === 1 ? 'device' : 'devices'}`;

// The status line under a switch.
export function gateSummary(status: GateLists): string {
  const parts: string[] = [];
  if (status.outdated.length > 0) {
    parts.push(`${devicesText(status.outdated.length)} ${status.outdated.length === 1 ? 'needs' : 'need'} to update the app: ${gateNames(status.outdated)}.`);
  }
  if (status.offline.length > 0) {
    parts.push(`${devicesText(status.offline.length)} did not answer, so they may be offline: ${gateNames(status.offline)}.`);
  }
  return parts.join(' ');
}

// Asks the owner to go ahead for the devices listed. Resolves to their ids, empty when nobody is listed, or null when
// the owner backed out. `seen` says what those people see until they update.
export async function confirmGate(status: GateLists, action: { title: string; okLabel: string; seen: string }): Promise<string[] | null> {
  const listed = [...status.offline, ...status.outdated];
  if (listed.length === 0) return [];
  const ok = await confirm({
    title: action.title,
    okLabel: action.okLabel,
    danger: status.outdated.length > 0,
    message: `${gateSummary(status)} ${action.seen} Updating the app catches them up.`,
  });
  return ok ? listed.map((d) => d.device) : null;
}
