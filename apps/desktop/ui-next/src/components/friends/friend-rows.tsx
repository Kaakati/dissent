// The rows of the Friends lists: a friend (presence, name, tag, status or call line, Message and More), a request (Accept
// and Ignore, or Cancel) and a blocked person (Unblock). Ported from friendRow, incomingRow, outgoingRow and blockedRow in
// the old UI's views/friends.js. A row opens the conversation; its buttons do not.
import type { MouseEvent, ReactNode } from 'react';
import { EllipsisVertical, MessageCircle } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { openConversationMenu } from '@/components/menus/conversation-menu';
import { PRESENCE_LABEL } from '@/components/presence';
import { UserAvatar } from '@/components/user-avatar';
import { addressOf, shortOf } from '@/lib/address';
import { acceptContact, cancelContactRequest, declineContact, unblockUser } from '@/lib/actions';
import { useShell } from '@/lib/store';
import type { Person } from '@/lib/types';
import { callOf, directOf, presenceOfPerson } from './friends-state';

// A button in a row: its click must not also open the conversation the row opens.
function stop(fn: () => void) {
  return (e: MouseEvent) => {
    e.stopPropagation();
    fn();
  };
}

// The name is bold and the tag shows on hover; the second line is the status.
function PersonText({ person, sub }: { person: Person; sub: string }) {
  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex min-w-0 items-baseline gap-s-1">
        <span className="truncate text-body leading-[19px] font-semibold text-text-strong">{person.display_name}</span>
        <span className="hidden shrink-0 font-mono text-meta leading-4 whitespace-nowrap text-muted-foreground group-focus-within:inline group-hover:inline" title={addressOf('user', person.root_id)}>
          {`·${shortOf('user', person.root_id)}`}
        </span>
      </div>
      <span className="truncate text-meta leading-4 text-muted-foreground">{sub}</span>
    </div>
  );
}

// A round action in a friend row: 36 px, the sidebar surface.
function RoundButton({ label, icon, onClick, haspopup }: { label: string; icon: ReactNode; onClick: (e: MouseEvent<HTMLButtonElement>) => void; haspopup?: boolean }) {
  return (
    <Tip label={label}>
      <button
        type="button"
        aria-label={label}
        aria-haspopup={haspopup ? 'menu' : undefined}
        onClick={onClick}
        className="grid size-9 shrink-0 place-items-center rounded-full bg-sidebar text-text transition-colors hover:bg-elevated-hover"
      >
        {icon}
      </button>
    </Tip>
  );
}

// A friend: avatar with presence, name, the call or status line, and Message and More. Hover or focus shows the buttons.
export function FriendRow({ person }: { person: Person }) {
  const presence = presenceOfPerson(person);
  const call = callOf(person.root_id);
  const open = () => void useShell.getState().openDm(person.root_id, person.display_name);
  return (
    <div
      className="group relative flex min-h-14 cursor-pointer items-center gap-s-3 border-t border-divider pr-s-2 pl-s-3 first:border-t-0 hover:bg-hover focus-within:bg-hover"
      onClick={open}
    >
      <UserAvatar name={person.display_name} seed={person.root_id} presence={presence} />
      <PersonText person={person} sub={call ? `In a call · ${call}` : PRESENCE_LABEL[presence]} />
      <div className="flex shrink-0 items-center gap-s-2 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
        <RoundButton label={`Message ${person.display_name}`} icon={<MessageCircle className="size-[18px]" aria-hidden="true" />} onClick={stop(open)} />
        <RoundButton
          label={`More actions for ${person.display_name}`}
          haspopup
          icon={<EllipsisVertical className="size-[18px]" aria-hidden="true" />}
          onClick={(e) => {
            e.stopPropagation();
            const r = e.currentTarget.getBoundingClientRect();
            openConversationMenu(r.left, r.bottom, directOf(person));
          }}
        />
      </div>
    </div>
  );
}

// An incoming request (Accept, Ignore) or an outgoing one (Cancel). Their buttons stay visible.
export function RequestRow({ person, incoming }: { person: Person; incoming: boolean }) {
  const sub = incoming
    ? (person.online ? 'Incoming request · Online' : 'Incoming request')
    : 'Outgoing request · Waiting for them';
  return (
    <div className="group flex min-h-14 items-center gap-s-3 border-t border-divider pr-s-2 pl-s-3 first:border-t-0">
      <UserAvatar name={person.display_name} seed={person.root_id} presence={presenceOfPerson(person)} />
      <PersonText person={person} sub={sub} />
      <div className="flex shrink-0 items-center gap-s-2">
        {incoming ? (
          <>
            <Button size="sm" className="bg-success text-white hover:bg-success/90" aria-label={`Accept ${person.display_name}`} onClick={() => void acceptContact(person.root_id)}>Accept</Button>
            <Button size="sm" variant="secondary" aria-label={`Ignore ${person.display_name}`} onClick={() => void declineContact(person.root_id)}>Ignore</Button>
          </>
        ) : (
          <Button size="sm" variant="secondary" aria-label={`Cancel request to ${person.display_name}`} onClick={() => void cancelContactRequest(person.root_id)}>Cancel</Button>
        )}
      </div>
    </div>
  );
}

// A blocked person: their name, and Unblock.
export function BlockedRow({ person }: { person: Person }) {
  return (
    <div className="group flex min-h-14 items-center gap-s-3 border-t border-divider pr-s-2 pl-s-3 first:border-t-0">
      <UserAvatar name={person.display_name} seed={person.root_id} presence="offline" />
      <PersonText person={person} sub="Blocked" />
      <div className="flex shrink-0 items-center gap-s-2">
        <Button size="sm" variant="secondary" aria-label={`Unblock ${person.display_name}`} onClick={() => void unblockUser(person.root_id)}>Unblock</Button>
      </div>
    </div>
  );
}
