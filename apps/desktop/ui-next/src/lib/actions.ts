// Actions for the dialogs, menus and the right column. Each calls the backend and then refreshes the store, as
// the old UI's actions.js does. A failure becomes a toast and the action resolves to undefined (falsy), so a caller never
// throws. Views that need a change to the shell go through useShell; these are the ones that talk to the engine.
import { api } from './api';
import { has } from './features';
import { refreshProfiles } from './profiles';
import { useShell } from './store';
import { toast } from './toasts';
import type { ChannelAccess, CommunitySummary, GroupSummary, Id, Trait } from './types';

async function guarded<T>(fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (err) {
    toast(err);
    return undefined;
  }
}

const shell = () => useShell.getState();

// ---- servers ----

export const createCommunity = (name: string) =>
  guarded(async () => {
    const summary = (await api.createCommunity(name)) as CommunitySummary;
    await shell().loadCommunities();
    await shell().openServer(summary.id);
    return summary;
  });

// Not guarded: the invite card shows a failed preview itself, so the error has to reach it.
export const invitePreview = (link: string) => api.invitePreview(link);

export const joinCommunity = (invite: string) =>
  guarded(async () => {
    const summary = (await api.joinCommunity(invite)) as CommunitySummary;
    await shell().loadCommunities();
    await shell().openServer(summary.id);
    return summary;
  });

export const leaveCommunity = (communityId: Id) =>
  guarded(async () => {
    await api.leaveCommunity(communityId);
    await shell().loadCommunities();
    return true;
  });

export const createInvite = (communityId: Id) => guarded(async () => (await api.createInvite(communityId)) as string);

export const createFullInvite = (communityId: Id) => guarded(() => api.createFullInvite(communityId));

export const loadInvites = (communityId: Id) => api.listInvites(communityId);

// Makes a text channel and opens it, or a voice channel, which stays unjoined until someone clicks it.
export const createChannel = (
  communityId: Id,
  name: string,
  { voice = false, access }: { voice?: boolean; access?: Pick<ChannelAccess, 'roles' | 'members'> } = {},
) =>
  guarded(async () => {
    if (voice) {
      await api.createVoiceChannel(communityId, name);
      await shell().loadCommunity(communityId);
      return true;
    }
    // A private channel is made in one command, so it is never listed to everyone.
    const channel = (access
      ? await api.createPrivateChannel(communityId, name, access)
      : await api.createChannel(communityId, name)) as { id: Id };
    await shell().loadCommunity(communityId);
    await shell().openChannel(channel.id);
    return true;
  });

// Sets who may read a channel. Owner and admin only; the engine checks it again.
export const setChannelAccess = (communityId: Id, channelId: Id, access: ChannelAccess) =>
  guarded(async () => {
    await api.setChannelAccess(communityId, channelId, access);
    await shell().loadCommunity(communityId);
    return true;
  });

// Changes only the fields in `patch`. Owner and admin only; the engine checks it again.
export const updateChannel = (communityId: Id, channelId: Id, patch: Parameters<typeof api.updateChannel>[2]) =>
  guarded(async () => {
    await api.updateChannel(communityId, channelId, patch);
    await shell().loadCommunity(communityId);
    return true;
  });

export const deleteChannel = (communityId: Id, channelId: Id) =>
  guarded(async () => {
    await api.deleteChannel(communityId, channelId);
    await shell().loadCommunity(communityId);
    return true;
  });

// A server's icon: set or cleared by an owner or admin. The server's view and the list are reloaded, so the rail updates.
export const setCommunityIcon = (communityId: Id, bytes: string) =>
  guarded(async () => {
    await api.setCommunityIcon(communityId, bytes);
    await shell().loadCommunities();
    await shell().loadCommunity(communityId);
    return true;
  });

export const clearCommunityIcon = (communityId: Id) =>
  guarded(async () => {
    await api.clearCommunityIcon(communityId);
    await shell().loadCommunities();
    await shell().loadCommunity(communityId);
    return true;
  });

// Server Settings. A rename reloads the list too, since the rail and the switcher show the name.
export const renameCommunity = (communityId: Id, name: string) =>
  guarded(async () => {
    await api.renameCommunity(communityId, name);
    await shell().loadCommunities();
    await shell().loadCommunity(communityId);
    return true;
  });

export const setCommunityBanner = (communityId: Id, bytes: string) =>
  guarded(async () => {
    await api.setCommunityBanner(communityId, bytes);
    await shell().loadCommunity(communityId);
    return true;
  });

export const clearCommunityBanner = (communityId: Id) =>
  guarded(async () => {
    await api.clearCommunityBanner(communityId);
    await shell().loadCommunity(communityId);
    return true;
  });

// Server Settings' Save Changes: only the parts that changed are published, one event each, then the server reloads.
export interface ServerProfileChanges {
  name?: string;
  banner?: { kind: 'image'; bytes: string } | { kind: 'gif'; gif: string; poster: string } | { kind: 'preset'; id: number } | null;
  profile?: { description: string; traits: Trait[] };
}

export const saveServerProfile = (communityId: Id, changes: ServerProfileChanges) =>
  guarded(async () => {
    if (changes.name !== undefined) await api.renameCommunity(communityId, changes.name);
    if (changes.banner !== undefined) {
      const b = changes.banner;
      if (b === null) await api.clearCommunityBanner(communityId);
      else if (b.kind === 'preset') await api.setCommunityBannerPreset(communityId, b.id);
      else if (b.kind === 'gif') await api.setCommunityBannerAnimated(communityId, b.gif, b.poster);
      else await api.setCommunityBanner(communityId, b.bytes);
    }
    if (changes.profile) await api.setCommunityProfile(communityId, changes.profile.description, changes.profile.traits);
    if (changes.name !== undefined) await shell().loadCommunities();
    await shell().loadCommunity(communityId);
    return true;
  });

export const claimHandle = (communityId: Id, handle: string) =>
  guarded(async () => {
    await api.claimHandle(communityId, handle);
    await shell().loadCommunity(communityId);
    toast(`Handle @${handle} claimed`, 'info');
    return true;
  });

// ---- contacts and direct messages ----

export const contactLink = () => guarded(() => api.contactLink());

// Adds a contact from an address or a contact link, then opens the new direct conversation.
export const addContact = (link: string) =>
  guarded(async () => {
    const user = (await api.addContact(link)) as { root_id: Id; display_name: string };
    await shell().loadConversations();
    await shell().openDm(user.root_id, user.display_name);
    return true;
  });

export const peerContactLink = (rootId: Id) => guarded(() => api.peerContactLink(rootId));

// Friend requests. Each refreshes the contact lists and the conversations, as the old UI's actions.js contactAction does.
const contactAction = (fn: () => Promise<unknown>) =>
  guarded(async () => {
    await fn();
    await Promise.all([shell().loadContacts(), shell().loadConversations()]);
    refreshProfiles();
    return true;
  });

export const sendContactRequest = (link: string) => contactAction(() => api.sendContactRequest(link));
export const acceptContact = (rootId: Id) => contactAction(() => api.acceptContact(rootId));
export const declineContact = (rootId: Id) => contactAction(() => api.declineContact(rootId));
export const cancelContactRequest = (rootId: Id) => contactAction(() => api.cancelContactRequest(rootId));
export const unblockUser = (rootId: Id) => contactAction(() => api.unblockUser(rootId));

export const userProfile = (user: Id) => guarded(() => api.userProfile(user));

// Moderation of a server member. The engine enforces the same rules as the profile card shows (see profile-card.tsx).
export const moderate = (communityId: Id, action: string, user: Id) =>
  guarded(async () => {
    await api.moderate(communityId, action, user);
    await shell().loadCommunity(communityId);
    return true;
  });

export const safetyNumber = (user: Id) => guarded(async () => (await api.safetyNumber(user)) as string);

export const markSafetyVerified = (user: Id) =>
  guarded(async () => {
    await api.markSafetyVerified(user);
    refreshProfiles();
    toast('Safety number marked as compared', 'info');
    return true;
  });

export const setNote = (peer: Id, kind: 'note' | 'nickname', text: string) =>
  guarded(async () => {
    if (!has('notes')) return false;
    await api.setNote(peer, kind, text);
    await shell().loadConversations();
    refreshProfiles();
    return true;
  });

export const removeContact = (rootId: Id) =>
  guarded(async () => {
    await api.removeContact(rootId);
    await shell().loadContacts();
    return true;
  });

export const blockUser = (rootId: Id) =>
  guarded(async () => {
    await api.blockUser(rootId);
    await Promise.all([shell().loadContacts(), shell().loadConversations()]);
    return true;
  });

export const setIgnored = (rootId: Id, ignore: boolean) =>
  guarded(async () => {
    if (ignore) await api.ignoreUser(rootId);
    else await api.unignoreUser(rootId);
    await shell().loadIgnored();
    return true;
  });

export const closeDm = (key: string) =>
  guarded(async () => {
    await api.closeConversation(key);
    await shell().loadConversations();
    return true;
  });

export const pinConversation = (key: string, pinned: boolean) =>
  guarded(async () => {
    await api.pinConversation(key, pinned);
    await shell().loadConversations();
    return true;
  });

// Clears the unread marker. The engine is told too when it keeps read markers (the pins feature).
export const markRead = (key: string) =>
  guarded(async () => {
    const conversation = key.startsWith('dm:') || key.startsWith('group:');
    if (has('pins') || conversation) await api.markRead(key);
    useShell.setState((s) => {
      const unread = new Set(s.unread);
      unread.delete(key);
      return { unread };
    });
    // The engine's unread count for the conversation changed, so the rail and the sidebar read it again.
    if (conversation) await useShell.getState().loadConversations();
    return true;
  });

// ---- groups ----

export const createGroup = (name: string, peers: Id[]) =>
  guarded(async () => {
    const conv = (await api.createGroup(name, peers)) as GroupSummary;
    await shell().loadConversations();
    await shell().openGroup(conv);
    return conv;
  });

export const leaveGroup = (groupId: Id) =>
  guarded(async () => {
    await api.leaveGroup(groupId);
    await shell().loadConversations();
    if (shell().group?.id === groupId) await shell().openHome();
    return true;
  });

export const groupInviteLink = (groupId: Id) => guarded(() => api.groupInviteLink(groupId));

// Sends a direct message to the open conversation, or to a server member's direct conversation by id.
export const sendDm = (peer: Id, text: string) =>
  guarded(async () => {
    await api.sendDm(peer, text);
    await shell().loadConversations();
    return true;
  });

// ---- files ----

// The place a shared file goes: a channel (with the seeder's choice) or a direct conversation.
export type FileTarget = { kind: 'channel'; communityId: Id; channelId: Id; seed: boolean } | { kind: 'dm'; peer: Id };

export const sendFiles = (paths: string[], text: string | null, target: FileTarget) =>
  guarded(async () => {
    if (target.kind === 'dm') {
      await api.sendDmFiles(target.peer, paths, text ?? '');
      await shell().loadConversations();
    } else {
      await api.sendFiles(target.communityId, target.channelId, paths, text ?? '', { seed: target.seed });
    }
    if (has('transfers')) await shell().loadTransfers();
    return true;
  });

// Settings > My identity: the profile banner is published at once, to every server and to contacts. Me reloads, and the
// open profile cards read the new one.
const afterProfileBanner = async () => {
  useShell.setState({ me: await api.me() });
  refreshProfiles();
  return true;
};
export const setProfileBanner = (bytes: string) => guarded(async () => (await api.setProfileBanner(bytes), afterProfileBanner()));
export const setProfileBannerPreset = (preset: number) => guarded(async () => (await api.setProfileBannerPreset(preset), afterProfileBanner()));
export const setProfileBannerAnimated = (gif: string, poster: string) =>
  guarded(async () => (await api.setProfileBannerAnimated(gif, poster), afterProfileBanner()));
export const clearProfileBanner = () => guarded(async () => (await api.clearProfileBanner(), afterProfileBanner()));
