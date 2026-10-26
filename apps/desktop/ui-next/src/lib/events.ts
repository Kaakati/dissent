// Subscribes the shell to the backend's events and reports input for auto-idle. Ported from the listeners in
// the old UI's main.js. Call startShellEvents() once, before boot(), as main.js did.
import { api, listenTo, onEngineEvent } from './api';
import { openReminderMessage, reminderFired } from '@/components/chat/jump';
import { alertForChannelMessage, alertForDm, alertForGroup, openNotifiedConversation } from './alerts';
import { attachmentProgress, clearMentions, reloadForEvent, startChat, useChat } from './chat';
import { has } from './features';
import { useShell, type ShellState } from './store';
import type { CallState, EngineEvent, TypingTarget, UpdateInfo, UserStatus } from './types';

// The alerts for a new message, and the reload of the open conversation when the event is for it.
function onMessageEvent(ev: EngineEvent): void {
  switch (ev.kind) {
    case 'message': {
      void alertForChannelMessage(ev).catch(() => {});
      // A mention arriving in the channel being read is seen at once, so its count does not climb.
      const open = useChat.getState().target;
      if (open?.kind === 'channel' && open.communityId === ev.community_id && open.channelId === ev.channel_id && document.hasFocus()) {
        void clearMentions(ev.community_id, ev.channel_id);
      }
      break;
    }
    case 'dm':
      void useShell.getState().loadConversations().then(() => alertForDm(ev.peer)).catch(() => {});
      break;
    case 'group':
      void useShell.getState().loadConversations().then(() => alertForGroup(ev.group_id)).catch(() => {});
      break;
    case 'reminder_due':
      reminderFired(ev.reminder);
      break;
    default:
      break;
  }
  void reloadForEvent(ev).catch(() => {});
}

// Input counts as activity for auto-idle. The engine ignores repeats, so the UI reports at most once every 30 seconds.
const ACTIVITY_EVERY_MS = 30_000;

function watchActivity(): void {
  if (!has('status')) return;
  let last = 0;
  const report = () => {
    const now = Date.now();
    if (now - last < ACTIVITY_EVERY_MS) return;
    last = now;
    api.recordActivity().catch(() => {});
  };
  for (const type of ['pointerdown', 'keydown', 'wheel'] as const) {
    window.addEventListener(type, report, { capture: true, passive: true });
  }
}

let started = false;

// Idempotent: React's development double-mount must not subscribe twice.
export function startShellEvents(): void {
  if (started) return;
  started = true;
  const shell = useShell.getState;
  // Errors from a listener that cannot be attached (no backend) are ignored: the shell still renders.
  startChat();
  onEngineEvent((ev) => {
    void shell().handleEngineEvent(ev);
    onMessageEvent(ev);
  }).catch(() => {});
  listenTo<Parameters<typeof attachmentProgress>[0]>('dissent://attachment-progress', (p) => attachmentProgress(p)).catch(() => {});
  listenTo<{ conv?: string } | null>('dissent://notification-click', (p) => {
    const conv = p?.conv;
    if (typeof conv === 'string' && conv.startsWith('reminder:')) void openReminderMessage(conv.slice('reminder:'.length)).catch(() => {});
    else void openNotifiedConversation(conv);
  }).catch(() => {});
  listenTo<UpdateInfo>('dissent://update-available', (info) => shell().setUpdate(info)).catch(() => {});
  listenTo<Parameters<ShellState['handleTransfersEvent']>[0]>('dissent://transfers', (p) => shell().handleTransfersEvent(p)).catch(() => {});
  listenTo<CallState | null>('dissent://call-state', (c) => shell().handleCallEvent(c)).catch(() => {});
  listenTo<UserStatus>('dissent://status-changed', (s) => shell().applyStatus(s)).catch(() => {});
  listenTo<{ community_id?: string } | null>('dissent://avatar-changed', (p) => void shell().avatarChanged(p)).catch(() => {});
  listenTo<{ target: TypingTarget; names: string[] }>('dissent://typing-changed', (p) => shell().applyTyping(p)).catch(() => {});
  watchActivity();
}
