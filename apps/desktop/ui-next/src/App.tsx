// The application shell: title bar, then the grid of server rail, sidebar, main area and right column, with the user
// panel floating over the first two. The layout and sizes are the current UI's (see shell.css and index.css).
import { Titlebar } from '@/components/shell/titlebar';
import { ServerRail } from '@/components/shell/server-rail';
import { Sidebar } from '@/components/shell/sidebar';
import { MainArea } from '@/components/shell/main-area';
import { RightPanel } from '@/components/shell/right-panel';
import { UserBar } from '@/components/shell/user-bar';
import { ChatHeader } from '@/components/chat/chat-header';
import { ChatBody, ChatOverlay } from '@/components/chat/chat-body';
import { EmptyPane } from '@/components/chat/chat-notices';
import { CallBody, CallHeader } from '@/components/call/call-body';
import { CallHost } from '@/components/call/call-host';
import { LobbyBody, LobbyHeader } from '@/components/call/lobby';
import { useCallBoard } from '@/components/call/call-mode';
import { TransfersAside, TransfersBody, TransfersTabs } from '@/components/transfers/transfers-page';
import { FriendsBody, FriendsTabs } from '@/components/friends/friends-page';
import { SavedBody } from '@/components/saved/saved-page';
import { ProfileCardHost } from '@/components/profile/profile-card';
import { ChannelSettingsHost } from '@/components/channel-settings/channel-settings';
import { ServerSettingsHost } from '@/components/server-settings/server-settings';
import { EmojiPickerHost } from '@/components/chat/emoji-picker';
import { DialogHost } from '@/components/dialogs/dialog-host';
import { QuickSwitcher } from '@/components/dialogs/quick-switcher';
import { KeyboardShortcuts } from '@/components/keyboard-shortcuts';
import { ContextMenuHost } from '@/components/menus/context-menu-host';
import { useRightColumn } from '@/components/right/right-column';
import { Onboarding } from '@/components/startup/onboarding';
import { SettingsPage } from '@/components/settings/settings-page';
import { Toaster } from '@/components/toaster';
import { TipProvider } from '@/components/ui/tip';
import { useShell } from '@/lib/store';

export default function App() {
  const page = useShell((s) => s.page);
  // The Transfers page fills the main column, and its storage panel takes the right column.
  const transfers = page === 'transfers';
  // The Friends page (the default view of Home) fills the main column too, and its Active Now column is the right one.
  const friends = page === 'contacts';
  // The Saved Messages page (Home) fills the main column; it has no right column.
  const saved = page === 'saved';
  // The chat shows the open conversation or server channel. A voice channel's board, a call here, or the lobby of a live
  // call takes its place (call-mode.ts decides which).
  const chatOpen = useShell((s) => !s.page && !s.voiceChannelId && (s.route.kind === 'server' || s.peer !== null));
  const board = useCallBoard();
  const right = useRightColumn();
  return (
    <TipProvider>
      <div className="flex h-[calc(100vh/var(--ui-zoom,1))] flex-col overflow-hidden bg-chat text-text">
        <Titlebar />
        <div className="app-shell" data-right={transfers ? 'members' : right.mode}>
          <ServerRail />
          <Sidebar />
          <MainArea
            header={board === 'call' ? <CallHeader /> : board === 'lobby' ? <LobbyHeader /> : !transfers && chatOpen ? <ChatHeader /> : undefined}
            headerExtra={transfers ? <TransfersTabs /> : friends ? <FriendsTabs /> : null}
            overlay={!transfers && chatOpen && board === null ? <ChatOverlay /> : null}
          >
            {transfers ? <TransfersBody /> : friends ? <FriendsBody /> : saved ? <SavedBody /> : board === 'call' ? <CallBody /> : board === 'lobby' ? <LobbyBody /> : chatOpen ? <ChatBody /> : <div className="flex min-h-0 items-center justify-center"><EmptyPane /></div>}
          </MainArea>
          <RightPanel>{transfers ? <TransfersAside /> : right.node}</RightPanel>
          <UserBar />
        </div>
        <SettingsPage />
        <ChannelSettingsHost />
        <ServerSettingsHost />
        <EmojiPickerHost />
        <Onboarding />
        <DialogHost />
        <CallHost />
        <ContextMenuHost />
        <ProfileCardHost />
        <QuickSwitcher />
        <KeyboardShortcuts />
        <Toaster />
      </div>
    </TipProvider>
  );
}
