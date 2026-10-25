// The profile card: a compact card next to a clicked name (a member, a message, a DM), kept inside the window. One card at
// a time; Escape and a click outside close it, and the focus goes back where it was. Ported from ui/views/profile.js.
// ProfileCardBody is the card's content, shared with the Contacts & privacy preview (preview hides the actions).
import { type ReactNode, useEffect, useLayoutEffect, useRef } from 'react';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import { Ban, MessageSquare, ShieldCheck, ShieldMinus, ShieldPlus, Undo2 } from 'lucide-react';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { Button } from '@/components/ui/button';
import { UserAvatar } from '@/components/user-avatar';
import { addressOf, shortOf } from '@/lib/address';
import { moderate } from '@/lib/actions';
import { useShell } from '@/lib/store';
import type { CommunityView, Presence, Role, UserProfileView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { bannerBackground } from './banner';
import { useUserProfile } from './use-user-profile';

const ROLE_LABEL: Partial<Record<Role, string>> = { owner: 'Owner', admin: 'Admin', moderator: 'Moderator' };
const ROLE_TEXT: Partial<Record<Role, string>> = {
  owner: 'text-[var(--role-owner)]',
  admin: 'text-[var(--role-admin)]',
  moderator: 'text-[var(--role-mod)]',
};

interface CardState {
  userId: string | null;
  x: number;
  y: number;
  opener: HTMLElement | null; // the element that had the focus before the card, which gets it back
}

const useProfileCardState = create<CardState>()(() => ({ userId: null, x: 0, y: 0, opener: null }));

// Opens the card at a point (the click). Opening a card while one is open keeps the first opener.
export function openProfileCard(x: number, y: number, userId: string): void {
  const { userId: open, opener } = useProfileCardState.getState();
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  useProfileCardState.setState({ userId, x, y, opener: open ? opener : focused });
}

export function closeProfileCard(): void {
  const { opener } = useProfileCardState.getState();
  useProfileCardState.setState({ userId: null, opener: null });
  if (opener?.isConnected) opener.focus();
}

// The presence of a view: its status, else online or offline; the member's when the view has neither.
function presenceOfView(view: UserProfileView, fallback?: Presence): Presence | undefined {
  if (view.status) return view.status;
  if (typeof view.online === 'boolean') return view.online ? 'online' : 'offline';
  return fallback;
}

// Moderation is offered inside an open server, to owner, admin and moderator, never on yourself or the owner. These rules
// mirror dissent-events; the engine enforces them too.
function moderationButtons(community: CommunityView | null, inServer: boolean, meId: string | undefined, userId: string): ReactNode[] {
  if (!inServer || !community || !['owner', 'admin', 'moderator'].includes(community.my_role ?? '') || userId === meId) return [];
  const member = community.members.find((m) => m.root_id === userId);
  if (!member || member.role === 'owner') return [];
  const mine = community.my_role;
  const target = member.role;
  const canBan = mine === 'owner' || (mine === 'admin' && target !== 'admin') || (mine === 'moderator' && target === 'member');
  const canModRole = (mine === 'owner' || mine === 'admin') && (target === 'member' || target === 'moderator');
  const canAdminRole = mine === 'owner';

  const act = (label: string, action: string, icon: ReactNode, danger = false) => (
    <Button
      key={action}
      size="sm"
      variant={danger ? 'destructive' : 'secondary'}
      className="flex-1"
      onClick={async () => {
        closeProfileCard();
        await moderate(community.id, action, userId);
      }}
    >
      {icon}
      {label}
    </Button>
  );
  const out: ReactNode[] = [];
  if (canBan) out.push(member.banned ? act('Unban', 'unban', <Undo2 />) : act('Ban', 'ban', <Ban />, true));
  if (canModRole) {
    out.push(target === 'moderator'
      ? act('Revoke Moderator', 'revoke_moderator', <ShieldMinus />)
      : act('Grant Moderator', 'grant_moderator', <ShieldPlus />));
  }
  if (canAdminRole) out.push(target === 'admin' ? act('Revoke Admin', 'revoke_admin', <ShieldMinus />) : act('Grant Admin', 'grant_admin', <ShieldPlus />));
  return out;
}

// The card's body: banner, avatar, name and tag, the handle in this server, the role, mutual servers, and the actions.
// `preview` shows the card as others see it, without the actions (Settings > Contacts & privacy).
export function ProfileCardBody({ view, userId, preview = false }: { view: UserProfileView; userId: string; preview?: boolean }) {
  const me = useShell((s) => s.me);
  const inServer = useShell((s) => s.route.kind === 'server');
  const community = useShell((s) => s.community);
  const member = community?.members.find((m) => m.root_id === userId) ?? null;
  const isMe = userId === me?.root_id;
  const mutual = view.mutual_communities.length;
  const presence = presenceOfView(view, member?.status);
  const actions: ReactNode[] = preview || isMe
    ? []
    : [
      <Button
        key="message"
        size="sm"
        className="flex-1"
        onClick={() => {
          closeProfileCard();
          void useShell.getState().openDm(userId, view.display_name);
        }}
      >
        <MessageSquare />
        Message
      </Button>,
      <Button
        key="verify"
        size="sm"
        variant="secondary"
        className="flex-1"
        onClick={() => {
          closeProfileCard();
          void openSafetyDialog(userId, { name: view.display_name, keyChangedMs: view.key_changed_ms });
        }}
      >
        <ShieldCheck />
        Verify safety number
      </Button>,
      ...(preview ? [] : moderationButtons(community, inServer, me?.root_id, userId)),
    ];

  return (
    <>
      <div className="h-[60px]" style={{ background: bannerBackground(userId) }} />
      <div className="relative px-s-4 pb-s-4">
        <UserAvatar
          name={view.display_name}
          seed={userId}
          size="xl"
          presence={presence}
          className="-mt-10 size-[88px] text-[27px] ring-[6px] ring-ink"
        />
        <div className="mt-s-2 text-head leading-6 font-bold break-words text-text-strong">{view.display_name}</div>
        <div className="mt-0.5 font-mono text-sm leading-5 break-all text-muted-foreground" title={addressOf('user', userId)}>
          {`${view.display_name}·${shortOf('user', userId)}`}
        </div>
        {member?.handle && community ? (
          <div className="mt-s-2 text-sm leading-5 font-medium break-words text-text">{`@${member.handle} in ${community.name}`}</div>
        ) : null}
        {member && ROLE_LABEL[member.role] ? (
          <div className="mt-s-2 flex flex-wrap gap-1.5">
            <span className={cn('inline-flex h-5 items-center rounded-full bg-border-strong px-s-2 text-meta font-semibold', ROLE_TEXT[member.role])}>
              {ROLE_LABEL[member.role]}
            </span>
          </div>
        ) : null}
        <div className="mt-s-2 text-meta text-muted-foreground">
          {mutual ? `${mutual} mutual server${mutual === 1 ? '' : 's'}` : 'No mutual servers'}
        </div>
        {actions.length ? <div className="mt-s-3 flex flex-wrap gap-s-2">{actions}</div> : null}
      </div>
    </>
  );
}

// The card itself: placed next to the click, moved back inside the window when it would overflow.
function ProfileCard({ userId }: { userId: string }) {
  const { x, y } = useProfileCardState(useShallow((s) => ({ x: s.x, y: s.y })));
  const load = useUserProfile(userId);
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const r = node.getBoundingClientRect();
    node.style.left = `${Math.min(Math.max(8, x + 12), window.innerWidth - r.width - 8)}px`;
    node.style.top = `${Math.min(Math.max(8, y - 12), window.innerHeight - r.height - 8)}px`;
  }, [x, y, load.status]);

  useEffect(() => {
    if (load.status === 'failed') closeProfileCard();
  }, [load.status]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closeProfileCard();
    };
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) closeProfileCard();
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('pointerdown', onDown, true);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('pointerdown', onDown, true);
    };
  }, []);

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="User profile"
      className="fixed z-30 w-[300px] overflow-hidden rounded-lg bg-ink shadow-[var(--shadow-2)]"
      style={{ left: 0, top: 0 }}
    >
      {load.status === 'ready' ? (
        <ProfileCardBody view={load.view} userId={userId} />
      ) : (
        <p className="p-s-4 text-sm text-muted-foreground">Loading…</p>
      )}
    </div>
  );
}

// Mounted once in App.tsx. Renders the card while one is open.
export function ProfileCardHost() {
  const userId = useProfileCardState((s) => s.userId);
  return userId ? <ProfileCard key={userId} userId={userId} /> : null;
}
