// Custom roles and bots (Server Settings, Roles). See `dissent_core::custom_roles`.
import type { Id } from './types';

export interface RoleView {
  id: Id;
  name: string;
  color: string; // '#rrggbb', or '' for none
  perms: string[];
  position: number;
  members: Id[];
}

export interface BotView {
  root_id: Id;
  name: string;
  joined: boolean;
}

export interface RolesView {
  roles: RoleView[]; // highest position first
  bots: BotView[];
  all_perms: string[];
  my_perms: string[];
  my_ceiling: number | null; // positions below this one may be given; null for moderators and above
  honored: boolean; // custom bits count on released kinds
}

export interface RoleDraft {
  name: string;
  color: string;
  perms: string[];
  position: number;
}

// What each permission lets a holder do, for the editor.
export const PERM_LABELS: Record<string, { label: string; hint: string }> = {
  manage_roles: { label: 'Manage roles', hint: 'Make, edit and give roles below their own, with permissions they hold' },
  manage_channels: { label: 'Manage channels', hint: 'Create, edit, order and delete channels' },
  manage_server: { label: 'Manage server', hint: 'Icon, banner, profile, relays and history' },
  manage_messages: { label: 'Manage messages', hint: "Delete other members' messages" },
  ban_members: { label: 'Ban members', hint: 'Ban and unban members below them' },
  manage_invites: { label: 'Manage invites', hint: 'Revoke invites' },
  mention_everyone: { label: 'Mention everyone', hint: 'Use @everyone' },
  bypass_slowmode: { label: 'Bypass slowmode', hint: 'Post without waiting for slowmode' },
  manage_files: { label: 'Manage files', hint: 'File categories, other members’ listings, reputation' },
  manage_bots: { label: 'Manage bots', hint: 'Add and remove bots' },
  manage_storage: { label: 'Manage storage', hint: 'Grant seeders storage' },
};

const invoke = <T>(cmd: string, args: Record<string, unknown>): Promise<T> => {
  const tauri = window.__TAURI__;
  if (!tauri) return Promise.reject(new Error('No backend: the app has not loaded the Tauri globals or the mock.'));
  return tauri.core.invoke<T>(cmd, args);
};

export const rolesApi = {
  view: (communityId: Id) => invoke<RolesView>('roles_view', { communityId }),
  create: (communityId: Id, d: RoleDraft) => invoke<Id>('create_role', { communityId, ...d }),
  update: (communityId: Id, roleId: Id, d: RoleDraft) => invoke<void>('update_role', { communityId, roleId, ...d }),
  remove: (communityId: Id, roleId: Id) => invoke<void>('delete_role', { communityId, roleId }),
  assign: (communityId: Id, userId: Id, roleId: Id, on: boolean) => invoke<void>('assign_role', { communityId, userId, roleId, on }),
  createBot: (communityId: Id, name: string, roleIds: Id[]) => invoke<{ root_id: Id; token: string }>('create_bot', { communityId, name, roleIds }),
  unregisterBot: (communityId: Id, userId: Id) => invoke<void>('unregister_bot', { communityId, userId }),
  setHonored: (communityId: Id, on: boolean) => invoke<void>('set_custom_role_perms', { communityId, on }),
};
