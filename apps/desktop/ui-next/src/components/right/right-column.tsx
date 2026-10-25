// The right column. It shows the Friends page's Active Now, the server member list, the member list of an open group, the
// DM profile panel, or nothing; the current UI's call details and lobby come with their batches. Ported from
// ui/views/right.js and ui/views/members.js. The shell's grid follows `mode` (see App.tsx and shell.css).
import type { ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { UserAvatar } from '@/components/user-avatar';
import { addressOf, shortOf } from '@/lib/address';
import { groupInviteLink, leaveGroup } from '@/lib/actions';
import { confirm } from '@/lib/dialog';
import { has } from '@/lib/features';
import { copyText } from '@/lib/clipboard';
import { toast } from '@/lib/toasts';
import { useRight } from '@/lib/right';
import { findAttachment, useChat } from '@/lib/chat';
import { FileDetailPanel } from './file-detail-panel';
import { DmPanel } from '@/components/dm/dm-panel';
import { FriendsAside } from '@/components/friends/friends-page';
import { CallDetails } from '@/components/call/call-body';
import { LobbyRoster } from '@/components/call/lobby';
import { useCallBoard } from '@/components/call/call-mode';
import { openProfileCard } from '@/components/profile/profile-card';
import { useShell } from '@/lib/store';
import type { CommunityView, GroupSummary, Member, Presence, Role } from '@/lib/types';
import { cn } from '@/lib/utils';

export type RightMode = 'none' | 'members' | 'wide' | 'profile';

const GROUPS: [string, Role][] = [
  ['Owner', 'owner'],
  ['Admins', 'admin'],
  ['Moderators', 'moderator'],
  ['Members', 'member'],
];

// The role colour of a name, as .member-name.role-* in style.css.
const ROLE_TEXT: Partial<Record<Role, string>> = {
  owner: 'text-[var(--role-owner)]',
  admin: 'text-[var(--role-admin)]',
  moderator: 'text-[var(--role-mod)]',
};

// A card with a header row and a scrolling body, as on the member-list boards.
function Card({ title, children, foot }: { title: string; children: ReactNode; foot?: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between gap-s-2 pr-s-2 pl-s-4 font-semibold text-sm text-text-strong shadow-[inset_0_-1px_0_var(--border)]">
        <h3 className="min-w-0 truncate">{title}</h3>
      </div>
      <div className="dissent-scroll flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-s-2 pb-s-2">{children}</div>
      {foot}
    </div>
  );
}

// The server's members by role, banned people left out. Each row: avatar with presence, name in its role colour, and
// the handle.
function ServerMembers({ community }: { community: CommunityView }) {
  const count = community.members.filter((m) => !m.banned).length;
  return (
    <Card title={`Members — ${count}`}>
      {GROUPS.map(([label, role]) => {
        const people = community.members.filter((m) => m.role === role && !m.banned);
        if (people.length === 0) return null;
        return (
          <div key={role} className="flex flex-col gap-0.5">
            <h3 className="flex h-10 shrink-0 items-end px-s-2 pb-0 pt-s-5 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase tabular-nums">{`${label} — ${people.length}`}</h3>
            {people.map((m) => (
              <MemberRow key={m.root_id} member={m} />
            ))}
          </div>
        );
      })}
    </Card>
  );
}

// A click opens the member's profile card next to the row.
function MemberRow({ member }: { member: Member }) {
  const presence: Presence | undefined = member.status ?? (typeof member.online === 'boolean' ? (member.online ? 'online' : 'offline') : undefined);
  return (
    <button
      type="button"
      onClick={(e) => openProfileCard(e.clientX, e.clientY, member.root_id)}
      className="flex h-11 w-full min-w-0 shrink-0 items-center gap-s-3 rounded-lg px-s-2 text-left text-ui font-medium text-text hover:bg-hover"
      title={`${member.display_name}·${addressOf('user', member.root_id)}`}
    >
      <UserAvatar name={member.display_name} seed={member.root_id} presence={presence} />
      <span className={cn('flex min-w-0 flex-1 items-center gap-1.5 truncate', ROLE_TEXT[member.role])}>{member.display_name}</span>
      {member.handle ? <span className="max-w-[40%] shrink-0 truncate text-meta text-muted-foreground">{`@${member.handle}`}</span> : null}
    </button>
  );
}

// Placeholder rows while the server's view loads.
function MemberSkeleton() {
  return (
    <div className="flex flex-col gap-s-2 p-s-2" aria-hidden="true">
      {[0, 1, 2, 3, 4].map((i) => (
        <div key={i} className="flex h-11 items-center gap-s-3 px-s-2">
          <span className="size-6 rounded-full bg-elevated" />
          <span className="h-3.5 flex-1 rounded-sm bg-elevated" />
        </div>
      ))}
    </div>
  );
}

// Members of an open group: a creator badge, a name·suffix tag under each name, and the invite and leave actions.
function GroupMembers({ group }: { group: GroupSummary }) {
  const members = group.members ?? [];
  const copyInvite = async () => {
    const link = await groupInviteLink(group.id);
    if (link && (await copyText(link))) toast('Group invite link copied', 'info');
  };
  const leave = async () => {
    if (await confirm({ message: `Leave ${group.name}? You'll stop receiving its messages.`, okLabel: 'Leave' })) await leaveGroup(group.id);
  };
  return (
    <Card
      title={`Members — ${members.length}`}
      foot={
        has('groups') ? (
          <div className="mt-auto flex flex-col gap-s-2 p-s-4">
            <Button variant="secondary" onClick={() => void copyInvite()} className="w-full">Copy group invite link</Button>
            <Button variant="outline" onClick={() => void leave()} className="w-full border-danger-text text-danger-text hover:bg-danger-tint">Leave group</Button>
          </div>
        ) : null
      }
    >
      {members.map((m) => (
        <div
          key={m.root_id}
          className="flex min-h-11 w-full min-w-0 shrink-0 items-center gap-s-3 rounded-lg px-s-2 py-1.5 text-left"
          title={`${m.display_name}·${addressOf('user', m.root_id)}`}
        >
          <span className="relative shrink-0">
            <UserAvatar name={m.display_name} seed={m.root_id} />
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="flex min-w-0 items-center gap-1.5 truncate text-ui font-medium text-text">
              <span className="truncate">{m.display_name}</span>
              {m.root_id === group.creator ? <span className="inline-flex h-[18px] items-center rounded-sm bg-warn-tint px-1.5 text-tiny font-semibold text-warn">Creator</span> : null}
            </span>
            <span className="truncate font-mono text-meta text-muted-foreground">{`${m.display_name.toLowerCase()}·${shortOf('user', m.root_id)}`}</span>
          </span>
        </div>
      ))}
    </Card>
  );
}

// What the column shows now, and the mode the shell's grid needs for it.
export function useRightColumn(): { mode: RightMode; node: ReactNode } {
  const board = useCallBoard();
  const callOfChannel = useShell((s) => s.call?.scope === 'channel');
  const fileDetail = useRight((s) => s.fileDetail);
  const attachmentOpen = useChat(() => (fileDetail ? findAttachment(fileDetail) : null));
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  const group = useShell((s) => s.group);
  const page = useShell((s) => s.page);
  const peer = useShell((s) => s.peer);
  const membersOpen = useRight((s) => s.membersOpen);
  const profileOpen = useRight((s) => s.profileOpen);

  // A voice channel's call shows its route and quality; a live call's lobby shows who is in it (right.js).
  if (board === 'call' && callOfChannel) return { mode: 'members', node: <CallDetails /> };
  if (board === 'lobby') return { mode: 'members', node: <LobbyRoster /> };
  // The transfer details of an attachment take the column while its message is on screen; the members come back after.
  if (fileDetail && attachmentOpen) return { mode: 'members', node: <FileDetailPanel id={fileDetail} /> };
  // The Friends page has its own column for Active Now (340 px, the wide grid).
  if (page === 'contacts') return { mode: 'wide', node: <FriendsAside /> };
  if (route.kind === 'server' && community && membersOpen) {
    return { mode: 'members', node: <ServerMembers community={community} /> };
  }
  if (route.kind === 'server' && membersOpen) {
    return { mode: 'members', node: <Card title="Members"><MemberSkeleton /></Card> };
  }
  if (route.kind === 'home' && group && membersOpen) {
    return { mode: 'members', node: <GroupMembers group={group} /> };
  }
  // A direct conversation's profile panel, while the panel toggle is on (the chat header's button).
  if (route.kind === 'home' && peer && !group && profileOpen) return { mode: 'profile', node: <DmPanel peer={peer} /> };
  return { mode: 'none', node: null };
}
