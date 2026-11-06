// What you can do to a person from their profile card, as data: the card renders these as buttons, and a member's
// right-click menu (right-column.tsx) as menu rows, so the two always offer the same actions under the same rules.
import { Ban, MessageSquare, ShieldCheck, ShieldMinus, ShieldPlus, Undo2, type LucideIcon } from 'lucide-react';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { moderate } from '@/lib/actions';
import { useShell } from '@/lib/store';
import type { CommunityView } from '@/lib/types';

export interface MemberAction {
  key: string;
  label: string;
  icon: LucideIcon;
  // The card draws the first action filled, the rest as secondary, and a danger one in red.
  primary?: boolean;
  danger?: boolean;
  run: () => void;
}

// Moderation is offered inside an open server, to owner, admin and moderator, never on yourself or the owner. These rules
// mirror dissent-events; the engine enforces them too.
function moderationActions(community: CommunityView | null, inServer: boolean, meId: string | undefined, userId: string): MemberAction[] {
  if (!inServer || !community || !['owner', 'admin', 'moderator'].includes(community.my_role ?? '') || userId === meId) return [];
  const member = community.members.find((m) => m.root_id === userId);
  if (!member || member.role === 'owner') return [];
  const mine = community.my_role;
  const target = member.role;
  const canBan = mine === 'owner' || (mine === 'admin' && target !== 'admin') || (mine === 'moderator' && target === 'member');
  const canModRole = (mine === 'owner' || mine === 'admin') && (target === 'member' || target === 'moderator');
  const canAdminRole = mine === 'owner';

  const act = (label: string, action: string, icon: LucideIcon, danger = false): MemberAction => ({
    key: action,
    label,
    icon,
    danger,
    run: () => void moderate(community.id, action, userId),
  });
  const out: MemberAction[] = [];
  if (canBan) out.push(member.banned ? act('Unban', 'unban', Undo2) : act('Ban', 'ban', Ban, true));
  if (canModRole) {
    out.push(target === 'moderator'
      ? act('Revoke Moderator', 'revoke_moderator', ShieldMinus)
      : act('Grant Moderator', 'grant_moderator', ShieldPlus));
  }
  if (canAdminRole) out.push(target === 'admin' ? act('Revoke Admin', 'revoke_admin', ShieldMinus) : act('Grant Admin', 'grant_admin', ShieldPlus));
  return out;
}

// The actions on `userId`, as the shell stands now. None on yourself.
export function memberActions(userId: string, person: { display_name: string; key_changed_ms?: number | null }): MemberAction[] {
  const { me, route, community } = useShell.getState();
  if (userId === me?.root_id) return [];
  return [
    {
      key: 'message',
      label: 'Message',
      icon: MessageSquare,
      primary: true,
      run: () => void useShell.getState().openDm(userId, person.display_name),
    },
    {
      key: 'verify',
      label: 'Verify safety number',
      icon: ShieldCheck,
      run: () => void openSafetyDialog(userId, { name: person.display_name, keyChangedMs: person.key_changed_ms }),
    },
    ...moderationActions(community, route.kind === 'server', me?.root_id, userId),
  ];
}
