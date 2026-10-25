// The message menu: a right click, the hover "…" button, or the Menu key and Shift+F10 on a focused message. Built from
// the shared menu model (lib/menu.ts) so it looks and keys the same as every other menu. Ported from
// ui/views/chat.js (openMessageMenu, removeMessage, copyMessage).
import { Copy, Link, Pencil, Reply, SmilePlus, Trash2 } from 'lucide-react';
import { copyText } from '@/lib/clipboard';
import { inviteLinks } from './invite-card';
import { confirm } from '@/lib/dialog';
import { useChat } from '@/lib/chat';
import { deleteMessage } from '@/lib/chat-actions';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { ChatTarget } from '@/lib/types';
import type { ChatView, ViewItem } from './use-chat-view';
import { pickReaction } from './reactions';

// Own messages, and in a server channel also for owners, admins and moderators (the roles of that server).
export function canDelete(item: ViewItem, view: ChatView): boolean {
  const role = useShell.getState().community?.my_role ?? null;
  const moderator = role !== null && ['owner', 'admin', 'moderator'].includes(role);
  return item.own || (!view.dm && moderator);
}

async function copyMessage(text: string, done: string): Promise<void> {
  if (await copyText(text)) toast(done, 'info');
}

// Shift+click (or Shift+Enter on the menu item) skips the confirm, as in Discord.
export async function removeMessage(target: ChatTarget, messageId: string, skipConfirm = false): Promise<void> {
  if (!skipConfirm && !(await confirm({ message: "Delete message? This can't be undone.", okLabel: 'Delete', danger: true }))) return;
  await deleteMessage(target, messageId);
}

// `row` is the message's element: the emoji picker returns the focus to it.
export function openMessageMenu(x: number, y: number, item: ViewItem, view: ChatView, row: HTMLElement | null): void {
  const live = !item.deleted;
  const main: MenuEntry[] = [
    view.social && live && row ? { label: 'Add reaction', icon: SmilePlus, onPick: () => pickReaction(view.target, item.id, item.reactions, row) } : null,
    view.social && live ? { label: 'Reply', icon: Reply, onPick: () => useChat.getState().startReply(item.id, item.authorName || 'unknown') } : null,
    live && item.text ? { label: 'Copy text', icon: Copy, onPick: () => void copyMessage(item.text, 'Text copied') } : null,
    live && inviteLinks(item.text).length === 1
      ? { label: 'Copy invite link', icon: Link, onPick: () => void copyMessage(inviteLinks(item.text)[0], 'Invite link copied') }
      : null,
    item.own && live ? { label: 'Edit message', icon: Pencil, onPick: () => useChat.getState().beginEdit(item.id) } : null,
    canDelete(item, view) && live
      ? { label: 'Delete message', icon: Trash2, danger: true, onPick: (e) => void removeMessage(view.target, item.id, e?.shiftKey) }
      : null,
  ];
  openContextMenu(x, y, [
    ...main,
    main.some(Boolean) ? 'sep' : null,
    { label: 'Copy message ID', icon: Copy, onPick: () => void copyMessage(item.id, 'Message ID copied') },
  ]);
}
