// The server menu: one definition for both places it opens, a right click (or Shift+F10) on a server icon in the rail,
// and the server's name at the top of the channel list (server-menu.tsx). Both show the same items to the same people.
// Each item is one Dissent can back. Mark As Read clears every text channel's unread dot and mention count. Invite to
// Server, Claim Handle and Copy Server Address are for everyone. Server Settings (with each section of the page),
// Create Channel and Server Icon are for owners and admins. Leave Server asks first and is for everyone but the owner,
// who must hand the server over or delete it instead. Help Forward Calls lets this device relay the server's voice
// calls while it is in none (off by default). Ported from the server context menu of the Discord reference, adapted
// to what the engine has: no emoji, stickers or events.
import type { MouseEvent } from 'react';
import { AtSign, Bell, BellOff, Copy, FolderPlus, Image as ImageIcon, LogOut, Plus, Settings, UserPlus } from 'lucide-react';
import { openCreateCategoryDialog } from '@/components/dialogs/category-dialogs';
import { openCreateChannelDialog } from '@/components/dialogs/create-channel-dialog';
import { openInviteDialog } from '@/components/dialogs/invite-dialogs';
import { openServerIconDialog } from '@/components/dialogs/server-icon-dialog';
import { openClaimHandleDialog } from '@/components/dialogs/small-dialogs';
import { openServerSettings, type Section } from '@/components/server-settings/server-settings';
import { addressOf } from '@/lib/address';
import { leaveCommunity, markRead } from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { asksLabel, loadStorageAsks } from '@/lib/storage-asks';
import { api } from '@/lib/api';
import { has } from '@/lib/features';
import { confirm } from '@/lib/dialog';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import {
  isServerMuted,
  loadServerPrefs,
  muteServer,
  muteSubtitle,
  notifyLabel,
  prefsOf,
  unmuteServer,
  updateServerPrefs,
} from '@/lib/server-prefs';
import { channelKey, useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { CommunityView, Id, ServerPrefs } from '@/lib/types';

// The community as the menu needs it: the open server's view, or the engine's for another server.
async function viewOf(communityId: Id): Promise<CommunityView> {
  const open = useShell.getState().community;
  return open?.id === communityId ? open : ((await api.communityView(communityId)) as CommunityView);
}

// Every text channel is read, and its mentions cleared with it.
async function markServerRead(view: CommunityView): Promise<void> {
  for (const ch of view.channels.filter((c) => !c.voice)) {
    await markRead(channelKey(view.id, ch.id));
    if (has('mentions')) await api.markMentionsRead(view.id, ch.id).catch(() => {});
  }
  await useShell.getState().refreshUnread();
}

// Opens a server's settings at a section. The page belongs to the open server, so that server is opened first.
async function openSettingsAt(communityId: Id, section: Section): Promise<void> {
  await useShell.getState().openServer(communityId);
  openServerSettings(communityId, section);
}

async function leave(view: CommunityView): Promise<void> {
  if (await confirm({ message: `Leave ${view.name}? You can rejoin with an invite link.`, okLabel: 'Leave' })) {
    await leaveCommunity(view.id);
  }
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

// Mute Server's submenu: timed mutes, until the user turns it back on, and Unmute while a mute is on.
function muteEntries(communityId: Id, muted: boolean): MenuEntry[] {
  const timed: [string, number][] = [
    ['For 15 Minutes', 15 * MINUTE],
    ['For 1 Hour', HOUR],
    ['For 3 Hours', 3 * HOUR],
    ['For 8 Hours', 8 * HOUR],
    ['For 24 Hours', 24 * HOUR],
  ];
  return [
    muted ? { label: 'Unmute Server', onPick: () => void unmuteServer(communityId) } : null,
    muted ? 'sep' : null,
    ...timed.map(([label, ms]) => ({ label, onPick: () => void muteServer(communityId, ms) })),
    { label: 'Until I turn it back on', onPick: () => void muteServer(communityId, null) },
  ];
}

// Notification Settings' submenu: which messages alert, and whether @everyone counts.
function notifyEntries(communityId: Id, prefs: ServerPrefs): MenuEntry[] {
  const levels: [ServerPrefs['notify'], string][] = [
    ['all', 'All Messages'],
    ['mentions', 'Only @mentions'],
    ['nothing', 'Nothing'],
  ];
  return [
    ...levels.map(([notify, label]) => ({
      label,
      radio: true,
      checked: prefs.notify === notify,
      onPick: () => void updateServerPrefs(communityId, { notify }),
    })),
    'sep',
    {
      label: 'Suppress @everyone',
      checkbox: true,
      checked: prefs.suppress_everyone,
      onPick: () => void updateServerPrefs(communityId, { suppress_everyone: !prefs.suppress_everyone }),
    },
  ];
}

// Opens the rail's menu at the pointer, or beside the icon when a key opened it.
export async function openServerRailMenu(e: MouseEvent<HTMLElement>, communityId: Id): Promise<void> {
  e.preventDefault();
  const r = e.currentTarget.getBoundingClientRect();
  await openServerMenu(communityId, e.clientX || r.right + 8, e.clientY || r.top);
}

// Opens the server menu at a point. The role comes from the server's view, read when the menu opens, so the items match
// what the user may do there now.
export async function openServerMenu(communityId: Id, x: number, y: number): Promise<void> {
  let view: CommunityView;
  try {
    view = await viewOf(communityId);
  } catch (err) {
    toast(err);
    return;
  }
  const withPrefs = has('server_prefs');
  if (withPrefs) await loadServerPrefs(communityId);
  const forwarding = has('forward_calls') ? await api.getForwardCalls(communityId).catch(() => null) : null;
  const prefs = prefsOf(communityId);
  const manage = view.my_role === 'owner' || view.my_role === 'admin';
  const asks = manage ? asksLabel((await loadStorageAsks(view.id)).length) : '';
  const entries: MenuEntry[] = [
    { label: 'Mark As Read', onPick: () => void markServerRead(view) },
    { label: 'Invite to Server', icon: UserPlus, onPick: () => openInviteDialog(view.id) },
    withPrefs
      ? {
          label: 'Mute Server',
          icon: BellOff,
          sub: muteSubtitle(prefs) || undefined,
          submenu: muteEntries(view.id, isServerMuted(prefs)),
        }
      : null,
    withPrefs
      ? { label: 'Notification Settings', icon: Bell, sub: notifyLabel(prefs), submenu: notifyEntries(view.id, prefs) }
      : null,
    withPrefs
      ? {
          label: 'Hide Muted Channels',
          checkbox: true,
          checked: prefs.hide_muted_channels,
          onPick: () => void updateServerPrefs(view.id, { hide_muted_channels: !prefs.hide_muted_channels }),
        }
      : null,
    forwarding !== null
      ? {
          label: 'Help Forward Calls',
          sub: 'Relay this server’s calls while you’re not in one. The call shows your name.',
          checkbox: true,
          checked: forwarding,
          onPick: () => void api.setForwardCalls(view.id, !forwarding).catch(toast),
        }
      : null,
    manage
      ? {
          label: 'Server Settings',
          icon: Settings,
          sub: asks || undefined,
          submenu: [
            // Every section of the page, in the page's order.
            { label: 'Server Profile', onPick: () => void openSettingsAt(view.id, 'profile') },
            { label: 'Boosts', onPick: () => void openSettingsAt(view.id, 'boosts') },
            { label: 'Members', sub: asks || undefined, onPick: () => void openSettingsAt(view.id, 'members') },
            { label: 'Invites', onPick: () => void openSettingsAt(view.id, 'invites') },
            { label: 'Bans', onPick: () => void openSettingsAt(view.id, 'bans') },
          ],
        }
      : null,
    manage ? { label: 'Create Channel', icon: Plus, onPick: () => openCreateChannelDialog(view.id) } : null,
    manage && has('channel_categories') ? { label: 'Create category', icon: FolderPlus, onPick: () => openCreateCategoryDialog() } : null,
    manage && has('avatars') ? { label: 'Server Icon', icon: ImageIcon, onPick: () => openServerIconDialog(view) } : null,
    { label: 'Claim Handle', icon: AtSign, onPick: () => openClaimHandleDialog(view.id) },
    {
      label: 'Copy Server Address',
      icon: Copy,
      onPick: async () => {
        const address = addressOf('server', view.id);
        if (address && (await copyText(address))) toast(`${view.name} address copied`, 'info');
      },
    },
    view.my_role !== 'owner' ? { label: 'Leave Server', icon: LogOut, danger: true, onPick: () => void leave(view) } : null,
  ];
  openContextMenu(x, y, entries);
}
