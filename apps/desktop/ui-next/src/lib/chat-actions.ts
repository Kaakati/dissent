// The chat's actions: sends, edits, deletes, reactions, file sends and attachment actions. Each calls the engine for the
// conversation it is in (a channel, a direct conversation or a group), then reloads that conversation. A failure is a
// toast and the action resolves to false, so a caller never throws. Ported from the message parts of the old UI's actions.js.
import { api } from './api';
import { confirm } from './dialog';
import { has } from './features';
import { markOpenRead, reloadOpen } from './chat';
import { reloadCallChat } from './call-chat';
import { noteReaction } from './quick-reactions';
import { extOf, guessMime, isExecutable } from './attachments';
import type { StagedFile } from './chat';
import { useShell } from './store';
import { toast } from './toasts';
import type { Attachment, ChatTarget, Id } from './types';

// Runs an engine call, reloads the conversation, and returns whether it worked. `read` marks the conversation read, for
// a message of ours (so no "New messages" line appears above it).
async function run(fn: () => Promise<unknown>, { read = true, lists = false }: { read?: boolean; lists?: boolean } = {}): Promise<boolean> {
  try {
    await fn();
  } catch (err) {
    toast(err);
    return false;
  }
  try {
    if (lists) await useShell.getState().loadConversations();
    await reloadOpen();
    if (read) markOpenRead();
  } catch (err) {
    toast(err);
  }
  // The call chat shows a conversation of its own, beside the open one, so it reads its messages again too.
  reloadCallChat();
  return true;
}

// Sends a text message to the conversation.
export function sendMessage(target: ChatTarget, text: string, mentions?: { users: Id[]; everyone: boolean }): Promise<boolean> {
  switch (target.kind) {
    case 'channel': return run(() => api.sendMessage(target.communityId, target.channelId, text, mentions));
    case 'dm': return run(() => api.sendDm(target.peer, text, mentions), { lists: true });
    case 'group': return run(() => api.sendGroupMessage(target.groupId, text, mentions), { lists: true });
  }
}

export function sendReply(target: ChatTarget, messageId: Id, text: string, mentions?: { users: Id[]; everyone: boolean }): Promise<boolean> {
  switch (target.kind) {
    case 'channel': return run(() => api.sendReply(target.communityId, target.channelId, messageId, text, mentions));
    case 'dm': return run(() => api.dmReply(target.peer, messageId, text, mentions), { lists: true });
    case 'group': return run(() => api.groupReply(target.groupId, messageId, text, mentions), { lists: true });
  }
}

export function editMessage(target: ChatTarget, messageId: Id, text: string): Promise<boolean> {
  switch (target.kind) {
    case 'channel': return run(() => api.editMessage(target.communityId, target.channelId, messageId, text), { read: false });
    case 'dm': return run(() => api.dmEdit(target.peer, messageId, text), { read: false });
    case 'group': return run(() => api.groupEdit(target.groupId, messageId, text), { read: false });
  }
}

export function deleteMessage(target: ChatTarget, messageId: Id): Promise<boolean> {
  switch (target.kind) {
    case 'channel': return run(() => api.deleteMessage(target.communityId, target.channelId, messageId), { read: false });
    case 'dm': return run(() => api.dmDelete(target.peer, messageId), { read: false });
    case 'group': return run(() => api.groupDelete(target.groupId, messageId), { read: false });
  }
}

// Adds the reaction, or removes it when `mine` says this user already reacted with it.
export function toggleReaction(target: ChatTarget, messageId: Id, emoji: string, mine: boolean): Promise<boolean> {
  if (!mine) noteReaction(emoji); // the quick row of the message menu counts what the viewer adds
  switch (target.kind) {
    case 'channel':
      return run(() => (mine ? api.unreact(target.communityId, target.channelId, messageId, emoji) : api.react(target.communityId, target.channelId, messageId, emoji)), { read: false });
    case 'dm':
      return run(() => (mine ? api.dmUnreact(target.peer, messageId, emoji) : api.dmReact(target.peer, messageId, emoji)), { read: false });
    case 'group':
      return run(() => (mine ? api.groupUnreact(target.groupId, messageId, emoji) : api.groupReact(target.groupId, messageId, emoji)), { read: false });
  }
}

// Sends files with an optional caption. A channel gets them with the seeder's choice; a direct conversation or a group
// gets them through the direct file path, as the current UI does.
// `scrub` is one "Remove identifying details" choice per path (lib/scrub.ts).
export function sendFiles(target: ChatTarget, paths: string[], text: string | null, mentions?: { users: Id[]; everyone: boolean }, scrub?: boolean[]): Promise<boolean> {
  return run(
    async () => {
      if (target.kind === 'channel') await api.sendFiles(target.communityId, target.channelId, paths, text ?? '', { seed: true, mentions, scrub });
      else await api.sendDmFiles(target.kind === 'dm' ? target.peer : target.groupId, paths, text ?? '', scrub);
      if (has('transfers')) await useShell.getState().loadTransfers();
    },
    { lists: target.kind !== 'channel' },
  );
}

// The native file dialog. The backend returns paths; a picked file may also come with its name and size.
export async function pickFiles(): Promise<{ path: string; name: string; size: number | null; mime: string }[]> {
  try {
    const picked = (await api.pickFiles()) as unknown[];
    return picked.map((f) => {
      const file = typeof f === 'string' ? { path: f } : (f as { path: string; name?: string; size?: number | null; mime?: string });
      const name = file.name ?? file.path.split(/[\\/]/).pop() ?? 'file';
      return { path: file.path, name, size: file.size ?? null, mime: file.mime ?? '' };
    });
  } catch (err) {
    toast(err);
    return [];
  }
}

const IMAGE_EXT: Record<string, string> = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp', 'image/bmp': 'bmp' };

// The name of a pasted picture, in local time: pasted-image-20261029-142233.png. The webview names a bitmap from the
// clipboard "image.png" (or nothing); a copied file keeps its own name.
export function pastedName(file: { name: string; type: string }, at = new Date()): string {
  if (file.name && !/^image\.\w+$/i.test(file.name)) return file.name;
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp = `${at.getFullYear()}${p(at.getMonth() + 1)}${p(at.getDate())}-${p(at.getHours())}${p(at.getMinutes())}${p(at.getSeconds())}`;
  return `pasted-image-${stamp}.${IMAGE_EXT[file.type] ?? 'png'}`;
}

function base64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}

// A pasted file crosses to the backend in memory, as base64. Above this it is not read: the attach button and drag and
// drop hand the backend a path instead.
export const MAX_PASTE_BYTES = 256 * 1024 ** 2;

// Files pasted into the composer. The webview has their bytes but no path, so the backend saves each one and it is staged
// as a picked file is.
export async function stagePasted(files: File[]): Promise<StagedFile[]> {
  const staged: StagedFile[] = [];
  if (files.some((f) => f.size > MAX_PASTE_BYTES)) toast('Large files: use the attach button or drag and drop');
  for (const file of files) {
    if (file.size > MAX_PASTE_BYTES) continue;
    const name = pastedName(file);
    const mime = file.type || guessMime(name);
    try {
      const saved = await api.stagePastedFile(name, base64(await file.arrayBuffer()));
      staged.push({ path: saved.path, name: saved.name, size: saved.size, mime: saved.mime || mime });
    } catch (err) {
      toast(err);
    }
  }
  return staged;
}

// A download of one attachment: the server id for a channel, none for a direct conversation.
export function downloadAttachment(communityId: Id | null, messageId: Id, attachment: Attachment): Promise<boolean> {
  return run(() => api.downloadAttachment(communityId, messageId, attachment.id), { read: false });
}

// Sends a message the recipient or an inbox refused, again. The backend keeps the packet for this.
export function retryMessage(messageId: Id): Promise<boolean> {
  return run(() => api.retryMessage(messageId), { read: false });
}

// Programs get a confirmation first. The check uses the real (last) extension.
export async function openAttachment(a: Attachment): Promise<void> {
  if (!a.local_path) return;
  if (isExecutable(a.name)) {
    const ok = await confirm({
      message: `"${a.name}" is a program (.${extOf(a.name)}). Opening it runs it on this computer. Only open programs from people you trust.`,
      okLabel: 'Open anyway',
    });
    if (!ok) return;
  }
  try {
    await api.openAttachment(a.local_path);
  } catch (err) {
    toast(err);
  }
}

export async function revealAttachment(path: string | null | undefined): Promise<void> {
  if (!path) return;
  try {
    await api.revealAttachment(path);
  } catch (err) {
    toast(err);
  }
}

