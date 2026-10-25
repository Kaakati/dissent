// The server's menu, opened from the name at the top of the channel list. Ported from toggleMenu in
// ui/views/sidebar.js. "Hide server details from seeders" comes with the member-only batch.
import type { ReactNode } from 'react';
import { AtSign, Copy, Image as ImageIcon, LogOut, Plus, UserPlus } from 'lucide-react';
import { openCreateChannelDialog } from '@/components/dialogs/create-channel-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { openServerIconDialog } from '@/components/dialogs/server-icon-dialog';
import { openClaimHandleDialog } from '@/components/dialogs/small-dialogs';
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { MENU_WIDTH, MenuItems } from '@/components/menus/menu-items';
import { addressOf } from '@/lib/address';
import { leaveCommunity } from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { has } from '@/lib/features';
import { confirm } from '@/lib/dialog';
import type { MenuEntry } from '@/lib/menu';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';

export function ServerMenu({ community, children }: { community: CommunityView; children: ReactNode }) {
  const manage = community.my_role === 'owner' || community.my_role === 'admin';
  const entries: MenuEntry[] = [
    { label: 'Create invite', icon: UserPlus, onPick: () => openInviteDialog(community.id) },
    { label: 'Claim handle', icon: AtSign, onPick: () => openClaimHandleDialog(community.id) },
    {
      label: 'Copy server address',
      icon: Copy,
      onPick: async () => {
        const address = addressOf('server', community.id);
        if (address && (await copyText(address))) toast(`${community.name} address copied`, 'info');
      },
    },
    manage ? { label: 'Create channel', icon: Plus, onPick: () => openCreateChannelDialog(community.id) } : null,
    has('avatars') && manage ? { label: 'Server icon', icon: ImageIcon, onPick: () => openServerIconDialog(community) } : null,
    {
      label: 'Leave server',
      icon: LogOut,
      danger: true,
      onPick: async () => {
        if (await confirm({ message: `Leave ${community.name}? You can rejoin with an invite link.`, okLabel: 'Leave' })) {
          await leaveCommunity(community.id);
        }
      },
    },
  ];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={4} className={MENU_WIDTH}>
        <MenuItems entries={entries} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
