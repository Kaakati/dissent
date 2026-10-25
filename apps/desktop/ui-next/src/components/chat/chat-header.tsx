// The chat header: the conversation's title (a channel, a person with their verified state, or a group with its members),
// the channel topic, the tools of a direct conversation (call, add to group, profile panel), the update button and the
// in-chat search. Ported from ui/views/chat.js (titleNodes, toolNodes, the search box).
import { Ellipsis, Hash, Phone, PanelRight, Search, UsersRound } from 'lucide-react';
import { openNewGroupDialog } from '@/components/dialogs/new-group-dialog';
import { UpdateButton } from '@/components/settings/update-dialog';
import { UserAvatar } from '@/components/user-avatar';
import { useChat, type PeerProfile } from '@/lib/chat';
import { startConversationCall } from '@/lib/calls';
import { has } from '@/lib/features';
import { toggleMembers, toggleProfile, useRight } from '@/lib/right';
import { openContextMenu } from '@/lib/menu';
import { useShell } from '@/lib/store';
import { IconBtn } from './parts';
import { useChatView, type ChatView } from './use-chat-view';

// A group's title: the two members who show first, the name, the member count and the "Members only" pill.
function GroupTitle({ view }: { view: ChatView }) {
  const group = useShell((s) => s.group);
  const members = group?.members ?? [];
  const count = members.length;
  const [a, b] = members.slice(0, 2);
  return (
    <>
      <span className="relative size-8 shrink-0">
        {a ? <UserAvatar name={a.display_name} seed={a.root_id} size="xs" className="absolute top-0 left-0 size-5 text-[7px]" /> : null}
        {b ? <UserAvatar name={b.display_name} seed={b.root_id} size="xs" className="absolute right-0 bottom-0 size-5 text-[7px] shadow-[0_0_0_2px_var(--chat)]" /> : null}
      </span>
      <h2 title={view.title}>{view.title}</h2>
      <span className="title-meta">{`${count} member${count === 1 ? '' : 's'}`}</span>
      <span className="pill ok">Members only</span>
    </>
  );
}

// A direct conversation's title: the person's picture and name, and "Verified" once the safety number was compared and the
// key has not changed since.
function PersonTitle({ view, profile }: { view: ChatView; profile: PeerProfile | null }) {
  const verified = has('safety') && Boolean(profile?.verified_ms) && !profile?.key_changed_ms;
  return (
    <>
      <UserAvatar name={view.title} seed={view.peer ?? undefined} size="sm" />
      <h2 title={view.title}>{view.title}</h2>
      {verified ? <span className="pill ok">Verified</span> : null}
    </>
  );
}

function Title({ view, profile }: { view: ChatView; profile: PeerProfile | null }) {
  if (view.group) return <GroupTitle view={view} />;
  if (view.dm) return <PersonTitle view={view} profile={profile} />;
  return (
    <>
      <Hash aria-hidden="true" />
      <h2 title={`#${view.title}`}>{view.title}</h2>
    </>
  );
}

// The buttons of a direct conversation (and of a group, which the current UI shows the same way).
function DmTools({ view, profile }: { view: ChatView; profile: PeerProfile | null }) {
  const profileOpen = useRight((s) => s.profileOpen);
  if (!view.peer) return null;
  const peer = view.peer;
  const who = view.title;
  const suffix = profile?.suffix ?? '';
  return (
    <>
      {has('calls') ? (
        <IconBtn
          label={view.group ? `Start a voice call in ${who}` : `Start voice call with ${who}`}
          icon={Phone}
          onClick={() => void startConversationCall(view.group ? { groupId: peer } : { peer })}
        />
      ) : null}
      {has('groups') ? (
        <IconBtn
          label="Add friends to group"
          icon={UsersRound}
          onClick={() => openNewGroupDialog({ preselect: [{ root_id: peer, display_name: who, suffix }] })}
        />
      ) : null}
      <IconBtn label={profileOpen ? 'Hide profile panel' : 'Show profile panel'} icon={PanelRight} pressed={profileOpen} onClick={toggleProfile} />
    </>
  );
}

// A channel's tools: the overflow menu that shows or hides the member list. The label is the current UI's.
export function MoreTools() {
  const membersOpen = useRight((s) => s.membersOpen);
  return (
    <IconBtn
      lg
      label="More group actions"
      title="More"
      icon={Ellipsis}
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openContextMenu(r.left, r.bottom + 4, [{ label: membersOpen ? 'Hide member list' : 'Show member list', onPick: toggleMembers }]);
      }}
    />
  );
}

// The in-chat search box. Esc clears it. The call and lobby headers use the same control.
export function HeaderSearch({ placeholder = 'Search' }: { placeholder?: string }) {
  const filter = useChat((s) => s.filter);
  return (
    <div className="search">
      <input
        type="search"
        placeholder={placeholder}
        aria-label="Search messages"
        autoComplete="off"
        value={filter}
        onChange={(e) => useChat.setState({ filter: e.target.value })}
        onKeyDown={(e) => {
          // Esc clears the search, and keeps the key from leaving the conversation while there is a search to clear.
          if (e.key === 'Escape' && filter) {
            e.preventDefault();
            e.stopPropagation();
            useChat.setState({ filter: '' });
          }
        }}
      />
      <Search aria-hidden="true" />
    </div>
  );
}

export function ChatHeader() {
  const view = useChatView();
  const profile = useChat((s) => s.peerProfile);
  const dmTools = view?.dm ? <DmTools view={view} profile={profile} /> : view ? <MoreTools /> : null;

  return (
    <header className="chat-header">
      <div className="chat-title">{view ? <Title view={view} profile={profile} /> : null}</div>
      {view?.topic ? (
        <div className="topic-wrap">
          <span className="chat-divider" />
          <span className="topic" title={view.topic}>{view.topic}</span>
        </div>
      ) : (
        // Without a topic the spacer pushes the tools, the update button and the search to the right edge.
        <span className="flex-1" aria-hidden="true" />
      )}
      <div className="header-tools">{dmTools}</div>
      <div className="update-host">
        <UpdateButton />
      </div>
      <HeaderSearch placeholder={view?.dm ? `Search ${view.title}` : 'Search'} />
    </header>
  );
}
