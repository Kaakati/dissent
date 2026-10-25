// The right-click menu on a conversation, and the New conversation menu on the Direct messages header. Items the engine
// does not support are left out, not disabled. Ported from openConversationMenu in ui/views/sidebar.js and the
// new-conversation menu there. The Start a call item comes with the calls batch.
import type { ReactNode } from 'react';
import { Copy, UserPlus, Users } from 'lucide-react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { openNewGroupDialog } from '@/components/dialogs/new-group-dialog';
import { openNoteDialog, openAddContactDialog } from '@/components/dialogs/small-dialogs';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { openProfileCard } from '@/components/profile/profile-card';
import { MENU_WIDTH, MenuItems } from '@/components/menus/menu-items';
import { addressOf } from '@/lib/address';
import {
  blockUser,
  closeDm,
  createInvite,
  groupInviteLink,
  leaveGroup,
  markRead,
  peerContactLink,
  pinConversation,
  removeContact,
  sendDm,
  setIgnored,
} from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { confirm } from '@/lib/dialog';
import { startConversationCall } from '@/lib/calls';
import { has } from '@/lib/features';
import { plural } from '@/lib/identity';
import { type MenuEntry, openContextMenu } from '@/lib/menu';
import { isMuted, muteConversation, unmuteConversation } from '@/lib/mutes';
import { convKey, useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { ConversationSummary } from '@/lib/types';

// `at` is the point the menu opens at: the Profile card opens next to it.
export function conversationItems(conv: ConversationSummary, at: { x: number; y: number }): MenuEntry[] {
  const s = useShell.getState();
  const group = conv.kind === 'group';
  const key = convKey(conv);
  const name = group ? conv.name : conv.display_name;
  const peer = group ? conv.id : conv.peer;
  const unread = s.unread.has(key);
  const pinned = Boolean(conv.pinned);
  const muted = isMuted(key);

  const items: MenuEntry[] = [
    { label: 'Mark as read', disabled: !unread, onPick: () => void markRead(key) },
    'sep',
    has('pins') ? { label: pinned ? 'Unpin' : 'Pin', onPick: () => void pinConversation(key, !pinned) } : null,
    has('pins') ? { label: pinned ? 'Remove from favorites' : 'Add to favorites', onPick: () => void pinConversation(key, !pinned) } : null,
    has('pins') ? 'sep' : null,
  ];

  if (!group) {
    items.push(
      { label: 'Profile', onPick: () => openProfileCard(at.x, at.y, conv.peer) },
      has('calls') ? { label: 'Start a call', onPick: () => void startConversationCall({ peer: conv.peer }) } : null,
      { label: 'Compare safety number', onPick: () => void openSafetyDialog(conv.peer, { name, keyChangedMs: null }) },
      has('notes') ? { label: 'Add note', sub: 'Only visible to you', onPick: () => openNoteDialog(conv.peer, name, 'note') } : null,
      has('notes') ? { label: 'Set nickname', onPick: () => openNoteDialog(conv.peer, name, 'nickname') } : null,
      has('contacts')
        ? {
          label: 'Copy contact link',
          onPick: async () => {
            const link = await peerContactLink(conv.peer);
            if (link && (await copyText(link))) toast(`${name}'s contact link copied`, 'info');
          },
        }
        : null,
      {
        label: 'Copy user address',
        icon: Copy,
        onPick: async () => {
          const address = addressOf('user', conv.peer);
          if (address && (await copyText(address))) toast(`${name}'s address copied`, 'info');
        },
      },
      has('close_dm') ? { label: 'Close DM', onPick: () => void closeDm(key) } : null,
      'sep',
      s.communities.length
        ? {
          label: 'Invite to server',
          submenu: s.communities.map((c) => ({
            label: c.name,
            onPick: async () => {
              const link = await createInvite(c.id);
              if (!link) return;
              await useShell.getState().openDm(conv.peer, name);
              if (await sendDm(conv.peer, `Join ${c.name}: ${link}`)) toast(`Invite to ${c.name} sent to ${name}`, 'info');
            },
          })),
        }
        : null,
      has('contacts')
        ? {
          label: 'Remove contact',
          onPick: async () => {
            if (await confirm({ message: `Remove ${name} from your contacts? Your conversation stays on this device.`, okLabel: 'Remove' })) {
              await removeContact(conv.peer);
            }
          },
        }
        : null,
      has('ignore')
        ? (s.ignored.includes(conv.peer)
          ? { label: 'Unignore', onPick: () => void setIgnored(conv.peer, false) }
          : { label: 'Ignore', sub: 'Hide their messages without telling them', onPick: () => void setIgnored(conv.peer, true) })
        : null,
      has('contacts')
        ? {
          label: 'Block',
          danger: true,
          onPick: async () => {
            if (await confirm({ message: `Block ${name}? They won't be told, and their messages stop reaching you.`, okLabel: 'Block' })) {
              await blockUser(conv.peer);
            }
          },
        }
        : null,
    );
  } else {
    items.push(
      has('groups')
        ? {
          label: 'Copy group invite link',
          onPick: async () => {
            const link = await groupInviteLink(conv.id);
            if (link && (await copyText(link))) toast('Group invite link copied', 'info');
          },
        }
        : null,
      has('groups')
        ? {
          label: 'Leave group',
          danger: true,
          onPick: async () => {
            if (await confirm({ message: `Leave ${name}? You'll stop receiving its messages.`, okLabel: 'Leave' })) await leaveGroup(peer);
          },
        }
        : null,
    );
  }

  if (has('mute')) {
    items.push(
      'sep',
      muted
        ? { label: `Unmute ${group ? name : `@${name}`}`, onPick: () => unmuteConversation(key) }
        : {
          label: `Mute ${group ? name : `@${name}`}`,
          submenu: [15, 60, 480, 0].map((mins) => ({
            label: mins === 0 ? 'Until I turn it back on' : mins >= 60 ? plural(mins / 60, 'hour') : `${mins} minutes`,
            onPick: () => muteConversation(key, mins === 0 ? null : Date.now() + mins * 60_000),
          })),
        },
    );
  }
  return items;
}

// Opens the conversation menu at a point (a right click, or Shift+F10 on the row).
export function openConversationMenu(x: number, y: number, conv: ConversationSummary): void {
  openContextMenu(x, y, conversationItems(conv, { x, y }));
}

// The "+" on the Direct messages header: a menu for a new conversation. Without groups there is only Add contact.
export function NewConversationMenu({ children }: { children: ReactNode }) {
  const entries: MenuEntry[] = [
    { label: 'Add contact', icon: UserPlus, onPick: () => openAddContactDialog() },
    has('groups') ? { label: 'Create a group', icon: Users, onPick: () => openNewGroupDialog() } : null,
  ];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent align="end" className={MENU_WIDTH}>
        <MenuItems entries={entries} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
