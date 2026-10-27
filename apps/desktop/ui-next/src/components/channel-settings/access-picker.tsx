// Who may read a private channel: a switch that makes the channel private, and a picker of roles and members.
// Owners and admins always see a private channel, so they are not in the picker. Banned members are left out.
import { cn } from '@/lib/utils';
import type { ChannelAccess, Member } from '@/lib/types';

export type AccessValue = Pick<ChannelAccess, 'roles' | 'members'>;

const ROLES: [ChannelAccess['roles'][number], string][] = [
  ['admin', 'Admins'],
  ['moderator', 'Moderators'],
];

const toggled = <T,>(list: T[], item: T): T[] => (list.includes(item) ? list.filter((x) => x !== item) : [...list, item]);

export function PrivateSwitch({
  checked,
  onChange,
  disabled = false,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className={cn('relative inline-flex h-6 w-10 shrink-0', disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer')}>
      <input
        type="checkbox"
        role="switch"
        className="peer sr-only"
        aria-label="Private Channel"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span
        aria-hidden="true"
        className="relative h-full w-full rounded-full bg-elevated shadow-[inset_0_0_0_1px_var(--border)] transition-colors after:absolute after:top-1 after:left-1 after:size-4 after:rounded-full after:bg-muted after:transition-transform peer-checked:bg-accent peer-checked:after:translate-x-4 peer-focus-visible:shadow-[0_0_0_2px_var(--accent)]"
      />
    </label>
  );
}

export function AccessPicker({
  members,
  value,
  onChange,
}: {
  members: Member[];
  value: AccessValue;
  onChange: (next: AccessValue) => void;
}) {
  const people = members.filter((m) => !m.banned);
  const box = 'size-4 shrink-0 cursor-pointer accent-accent';
  return (
    <div className="flex flex-col gap-s-4">
      <fieldset className="flex flex-col gap-s-2">
        <legend className="mb-s-2 text-sm leading-[19px] font-semibold text-text-strong">Roles</legend>
        {ROLES.map(([role, label]) => (
          <label key={role} className="flex cursor-pointer items-center gap-s-2 text-ui text-text">
            <input
              type="checkbox"
              className={box}
              checked={value.roles.includes(role)}
              onChange={() => onChange({ ...value, roles: toggled(value.roles, role) })}
            />
            {label}
          </label>
        ))}
      </fieldset>
      <fieldset className="flex flex-col gap-s-2">
        <legend className="mb-s-2 text-sm leading-[19px] font-semibold text-text-strong">Members</legend>
        {people.length === 0 ? <p className="text-sm text-muted-foreground">No members to add yet.</p> : null}
        {people.map((m: Member) => {
          const checked = value.members.includes(m.root_id);
          return (
            <label key={m.root_id} className="flex cursor-pointer items-center gap-s-2 text-ui text-text">
              <input
                type="checkbox"
                className={box}
                checked={checked}
                onChange={() => onChange({ ...value, members: toggled(value.members, m.root_id) })}
              />
              <span className="truncate">{m.display_name}</span>
            </label>
          );
        })}
      </fieldset>
    </div>
  );
}
