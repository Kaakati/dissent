// The message menu: a right click, the hover "…" button, or the Menu key and Shift+F10 on a focused message. Built from
// the shared menu model (lib/menu.ts) so it looks and keys the same as every other menu. Ported from
// the old UI's views/chat.js (openMessageMenu, removeMessage, copyMessage), with the saved, reminder and forward items.
import { createElement } from 'react';
import { AlarmClock, Bookmark, BookmarkX, Copy, Forward, Link, Mail, Pencil, Pin, PinOff, Reply, SmilePlus, Trash2, Volume2 } from 'lucide-react';
import { api } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { inviteLinks } from './invite-card';
import { confirm } from '@/lib/dialog';
import { useChat } from '@/lib/chat';
import { deleteMessage, toggleReaction } from '@/lib/chat-actions';
import { EMOJI, shortcodeOf } from '@/lib/emoji';
import { openContextMenu, type MenuEntry } from '@/lib/menu';
import { quickReactions } from '@/lib/quick-reactions';
import { markUnreadFrom, reminderChoices, refOf, saveMessage, savedCopy, setReminder, unsaveMessage } from '@/lib/saved';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { ChatTarget, MessageRef } from '@/lib/types';
import type { ChatView, ViewItem } from './use-chat-view';
import { openForwardDialog } from './forward-dialog';
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

// The link to a message, as the message link control reads it (see components/chat/message-link.tsx).
export function linkOf(target: ChatTarget, messageId: string): string {
  if (target.kind === 'channel') return `dissent://message/${target.communityId}/${target.channelId}/${messageId}`;
  if (target.kind === 'dm') return `dissent://message/dm/${target.peer}/${messageId}`;
  return `dissent://message/group/${target.groupId}/${messageId}`;
}

// Adds the reaction, or removes it when this user already has it.
function react(view: ChatView, item: ViewItem, emoji: string): void {
  const mine = item.reactions?.some((r) => r.emoji === emoji && r.me) ?? false;
  void toggleReaction(view.target, item.id, emoji, mine);
}

// Whether a channel message is pinned on this device. Pins are kept per device, so the list is read here.
async function pinnedHere(view: ChatView, messageId: string): Promise<boolean> {
  if (view.target.kind !== 'channel') return false;
  try {
    const pins = (await api.pinnedMessages(view.target.communityId, view.target.channelId)) as { id: string }[];
    return pins.some((m) => m.id === messageId);
  } catch {
    return false;
  }
}

async function setPinned(target: ChatTarget, messageId: string, pinned: boolean): Promise<void> {
  if (target.kind !== 'channel') return;
  try {
    await api.pinMessage(target.communityId, target.channelId, messageId, pinned);
  } catch (err) {
    toast(err);
    return;
  }
  toast(pinned ? 'Message pinned' : 'Message unpinned', 'info');
}

// Reads a message aloud. Any speech in progress stops first.
function speak(text: string): void {
  const synth = window.speechSynthesis;
  synth.cancel();
  synth.speak(new SpeechSynthesisUtterance(text));
}

// The emoji submenu: every picker emoji by its shortcode, then the full picker.
function reactionSubmenu(view: ChatView, item: ViewItem, row: HTMLElement): MenuEntry[] {
  return [
    ...EMOJI.map(([emoji]) => ({
      label: shortcodeOf(emoji),
      leading: createElement('span', { 'aria-hidden': 'true' }, emoji),
      onPick: () => react(view, item, emoji),
    })),
    'sep',
    { label: 'More…', icon: SmilePlus, onPick: () => pickReaction(view.target, item.id, item.reactions, row) },
  ];
}

// `row` is the message's element: the emoji picker returns the focus to it. Async only for the pin and saved lookups,
// which read this device's lists; the menu opens after them.
export async function openMessageMenu(x: number, y: number, item: ViewItem, view: ChatView, row: HTMLElement | null): Promise<void> {
  const live = !item.deleted;
  const ref: MessageRef = refOf(view.target, item);
  const [pinned, saved] = await Promise.all([
    live && !view.dm ? pinnedHere(view, item.id) : false,
    live ? savedCopy(ref) : undefined,
  ]);
  const reacts = view.social && live && row;
  const main: MenuEntry[] = [
    reacts ? { quick: quickReactions().map((emoji) => ({ emoji, label: shortcodeOf(emoji), onPick: () => react(view, item, emoji) })) } : null,
    reacts ? { label: 'Add Reaction', icon: SmilePlus, submenu: reactionSubmenu(view, item, row), scroll: true } : null,
    item.own && live ? { label: 'Edit Message', icon: Pencil, onPick: () => useChat.getState().beginEdit(item.id) } : null,
    view.social && live ? { label: 'Reply', icon: Reply, onPick: () => useChat.getState().startReply(item.id, item.authorName || 'unknown') } : null,
    live ? { label: 'Forward', icon: Forward, onPick: () => openForwardDialog(item.authorName, item.text) } : null,
    live ? { label: 'Copy Text', icon: Copy, disabled: !item.text, onPick: () => void copyMessage(item.text, 'Text copied') } : null,
    live && !view.dm && view.target.kind === 'channel'
      ? { label: pinned ? 'Unpin Message' : 'Pin Message', icon: pinned ? PinOff : Pin, onPick: () => void setPinned(view.target, item.id, !pinned) }
      : null,
    live
      ? saved
        ? { label: 'Remove Bookmark', icon: BookmarkX, onPick: () => void unsaveMessage(saved.key) }
        : { label: 'Bookmark Message', icon: Bookmark, onPick: () => void saveMessage(ref) }
      : null,
    live
      ? {
        label: 'Create Reminder',
        icon: AlarmClock,
        submenu: reminderChoices().map((c) => ({ label: c.label, onPick: () => void setReminder(ref, c.at) })),
      }
      : null,
    live ? { label: 'Mark Unread', icon: Mail, onPick: () => void markUnreadFrom(view.target, item.ms) } : null,
    live ? { label: 'Copy Message Link', icon: Link, onPick: () => void copyMessage(linkOf(view.target, item.id), 'Message link copied') } : null,
    live && 'speechSynthesis' in window ? { label: 'Speak Message', icon: Volume2, disabled: !item.text, onPick: () => speak(item.text) } : null,
    canDelete(item, view) && live
      ? { label: 'Delete Message', icon: Trash2, danger: true, onPick: (e) => void removeMessage(view.target, item.id, e?.shiftKey) }
      : null,
  ];
  const invite = live && inviteLinks(item.text).length === 1 ? inviteLinks(item.text)[0] : null;
  openContextMenu(x, y, [
    ...main,
    main.some(Boolean) ? 'sep' : null,
    invite ? { label: 'Copy invite link', icon: Link, onPick: () => void copyMessage(invite, 'Invite link copied') } : null,
    { label: 'Copy message ID', icon: Copy, onPick: () => void copyMessage(item.id, 'Message ID copied') },
  ]);
}
