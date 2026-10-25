// Global keyboard shortcuts, as ui/keyboard.js has them: Ctrl/Cmd+K quick switcher, Ctrl/Cmd+, Settings, Alt+Up/Down
// between channels or conversations, Alt+Shift+Up/Down to the next or previous unread, Esc in a DM or group back to
// Friends, and Up/Down through the server rail. Mounted once, in App.
import { useEffect } from 'react';
import { useDialogs } from '@/lib/dialog';
import { useContextMenu } from '@/lib/menu';
import { convKey, useShell } from '@/lib/store';
import { setSwitcherOpen, useSwitcher } from '@/components/dialogs/quick-switcher';
import type { ConversationSummary } from '@/lib/types';

interface Entry {
  key: string;
  active: boolean;
  open: () => void;
}

// The conversations in the current context, in the order the sidebar shows them.
function entries(): Entry[] {
  const s = useShell.getState();
  const { community } = s;
  if (s.route.kind === 'server' && community) {
    return community.channels
      .filter((c) => !c.voice)
      .map((ch) => ({
        key: `ch:${community.id}:${ch.id}`,
        active: s.channelId === ch.id && !s.voiceChannelId && !s.page,
        open: () => void s.openChannel(ch.id),
      }));
  }
  return s.conversations.map((conv: ConversationSummary) => ({
    key: convKey(conv),
    active: s.route.kind === 'home' && !s.page && s.peer === (conv.kind === 'group' ? conv.id : conv.peer),
    open: () => void s.openConversation(conv),
  }));
}

function step(dir: 1 | -1): void {
  const list = entries();
  if (list.length === 0) return;
  const current = list.findIndex((x) => x.active);
  const next = current < 0 ? (dir > 0 ? 0 : list.length - 1) : (current + dir + list.length) % list.length;
  list[next].open();
}

function jumpUnread(dir: 1 | -1): void {
  const list = entries();
  if (list.length === 0) return;
  const unread = useShell.getState().unread;
  const start = list.findIndex((x) => x.active);
  for (let i = 1; i <= list.length; i++) {
    const idx = start < 0
      ? (dir > 0 ? i - 1 : list.length - i)
      : (start + dir * i + list.length * 2) % list.length;
    if (unread.has(list[idx].key)) {
      list[idx].open();
      return;
    }
  }
}

// Any open overlay keeps its own Escape: a dialog, a menu, the switcher, Settings or the channel settings page.
function overlayOpen(): boolean {
  if (useDialogs.getState().dialogs.length > 0 || useDialogs.getState().confirm) return true;
  if (useContextMenu.getState().menu || useSwitcher.getState().open) return true;
  return document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"], [data-overlay]') !== null;
}

// Up and Down move focus between the rail's buttons, wrapping at the ends. Enter or Space activates, as on any button.
function onRailKey(e: KeyboardEvent): void {
  if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
  const rail = (e.target as HTMLElement | null)?.closest('nav[aria-label="Servers and direct messages"]');
  if (!rail) return;
  const buttons = [...rail.querySelectorAll<HTMLButtonElement>('button')];
  const i = buttons.indexOf(e.target as HTMLButtonElement);
  if (i < 0) return;
  e.preventDefault();
  const next = (i + (e.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length;
  buttons[next].focus();
}

function onKey(e: KeyboardEvent): void {
  const s = useShell.getState();
  // Esc in a DM or group leaves it for Friends. Overlays keep their own Escape.
  if (e.key === 'Escape' && !e.defaultPrevented && s.route.kind === 'home' && (s.peer || s.group) && !overlayOpen()) {
    e.preventDefault();
    void s.openHome();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    setSwitcherOpen(true);
    return;
  }
  if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key === ',') {
    e.preventDefault();
    s.openSettings();
    return;
  }
  if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) {
    onRailKey(e);
    return;
  }
  e.preventDefault();
  const dir = e.key === 'ArrowUp' ? -1 : 1;
  if (e.shiftKey) jumpUnread(dir);
  else step(dir);
}

export function KeyboardShortcuts() {
  useEffect(() => {
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);
  return null;
}
