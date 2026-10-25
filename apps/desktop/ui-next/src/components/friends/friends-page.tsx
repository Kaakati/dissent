// The Friends page, the default view of Home (views/friends.js). Its tabs and Add Friend sit in the header; the body is
// the search and the lists, or the Add Friend form; Active Now is the right column. Lists are the current UI's: friends
// online or all, pending requests (with the count on the tab), and blocked people when there are any.
import { Fragment, type ReactNode } from 'react';
import { Info, Users } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { PRESENCE_LABEL } from '@/components/presence';
import { UserAvatar } from '@/components/user-avatar';
import { matchesSearch } from '@/lib/address';
import { useShell } from '@/lib/store';
import type { ContactsView, Person } from '@/lib/types';
import { cn } from '@/lib/utils';
import { AddFriend } from './add-friend';
import { BlockedRow, FriendRow, RequestRow } from './friend-rows';
import { type FriendsTab, callOf, isUp, presenceOfPerson, useFriendsView } from './friends-state';

const EMPTY: ContactsView = { contacts: [], incoming: [], outgoing: [], blocked: [] };

// The contacts, read from the store. Until they load the lists are empty.
function useContacts(): ContactsView {
  return useShell((s) => s.contacts) ?? EMPTY;
}

// Online, All, Pending, and Blocked when someone is blocked. The tab list keeps the same order as the current UI.
function tabsFor(data: ContactsView): { key: FriendsTab; label: string }[] {
  const tabs: { key: FriendsTab; label: string }[] = [
    { key: 'online', label: 'Online' },
    { key: 'all', label: 'All' },
    { key: 'pending', label: 'Pending' },
  ];
  if (data.blocked.length) tabs.push({ key: 'blocked', label: 'Blocked' });
  return tabs;
}

// The tab, unless it is not offered now (the blocked tab once nobody is blocked), then Online.
function activeTab(data: ContactsView, tab: FriendsTab): FriendsTab {
  return tabsFor(data).some((t) => t.key === tab) ? tab : 'online';
}

const EMPTY_COPY: Record<FriendsTab, [string, string]> = {
  online: ['Nobody’s online', 'Friends appear here when their app is running.'],
  all: ['No friends yet', 'Add someone with their contact link.'],
  pending: ['No pending requests', 'Requests you send or receive show up here.'],
  blocked: ['Nobody is blocked', 'People you block can’t message you or send requests.'],
};

// The tabs sit in the page header, after the title: a dot, the tab buttons and Add Friend.
export function FriendsTabs() {
  const data = useContacts();
  const tab = useFriendsView((s) => s.tab);
  const addOpen = useFriendsView((s) => s.addOpen);
  const setTab = useFriendsView((s) => s.setTab);
  const setAddOpen = useFriendsView((s) => s.setAddOpen);
  const current = activeTab(data, tab);
  const incoming = data.incoming.length;
  return (
    <Fragment>
      <span className="mx-s-2 size-1 shrink-0 rounded-full bg-faint" aria-hidden="true" />
      <div role="tablist" aria-label="Friends" className="flex items-center gap-s-2">
        {tabsFor(data).map((t) => {
          const on = current === t.key && !addOpen;
          return (
            <button
              key={t.key}
              type="button"
              role="tab"
              aria-selected={on}
              onClick={() => setTab(t.key)}
              className={cn(
                'inline-flex h-9 items-center gap-s-2 rounded-lg px-s-3 text-ui font-medium transition-colors',
                on ? 'bg-selected text-text-strong' : 'text-muted-foreground hover:bg-hover hover:text-text',
              )}
            >
              {t.label}
              {t.key === 'pending' && incoming ? (
                <span className="grid h-4 min-w-4 place-items-center rounded-full bg-primary px-1 text-tiny font-bold text-primary-foreground tabular-nums" aria-label={`${incoming} incoming requests`}>
                  {incoming}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
      <button
        type="button"
        aria-pressed={addOpen}
        onClick={() => setAddOpen(true)}
        className={cn(
          'ml-s-2 inline-flex h-9 shrink-0 items-center rounded-full px-s-4 text-sm font-semibold transition-colors',
          addOpen ? 'bg-accent-tint text-accent-text' : 'bg-primary text-primary-foreground hover:bg-accent-hover',
        )}
      >
        Add Friend
      </button>
    </Fragment>
  );
}

// A heading over one section of rows, as the current UI's list-section.
function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col">
      <h3 className="mt-s-4 mb-s-2 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">{title}</h3>
      <div className="flex flex-col">{children}</div>
    </section>
  );
}

function Empty({ tab }: { tab: FriendsTab }) {
  const [title, text] = EMPTY_COPY[tab];
  return (
    <div className="mx-auto mt-16 flex max-w-90 flex-col items-center gap-1.5 text-center text-sm leading-[19px] text-muted-foreground">
      <Users className="size-10 text-faint" aria-hidden="true" />
      <span className="text-body font-semibold text-text">{title}</span>
      <span>{text}</span>
    </div>
  );
}

// The rows of the active tab, under one heading, filtered by the search box.
function FriendLists({ data, tab, query }: { data: ContactsView; tab: FriendsTab; query: string }) {
  const q = query.trim().toLowerCase();
  const match = (p: Person) => matchesSearch(p, q);
  const incoming = data.incoming.filter(match);
  const outgoing = data.outgoing.filter(match);
  const blocked = data.blocked.filter(match);
  const people = data.contacts.filter(match);

  if (tab === 'pending') {
    const count = incoming.length + outgoing.length;
    return count ? (
      <>
        {data.incoming.length > 0 ? <PendingTip /> : null}
        <Section title={`Pending — ${count}`}>
          {incoming.map((p) => <RequestRow key={`in-${p.root_id}`} person={p} incoming />)}
          {outgoing.map((p) => <RequestRow key={`out-${p.root_id}`} person={p} incoming={false} />)}
        </Section>
      </>
    ) : (
      <Empty tab={tab} />
    );
  }
  if (tab === 'blocked') {
    return blocked.length ? (
      <Section title={`Blocked — ${blocked.length}`}>
        {blocked.map((p) => <BlockedRow key={p.root_id} person={p} />)}
      </Section>
    ) : (
      <Empty tab={tab} />
    );
  }
  const picked = tab === 'online' ? people.filter(isUp) : people;
  const title = tab === 'online' ? 'Online' : 'All friends';
  return picked.length ? (
    <Section title={`${title} — ${picked.length}`}>
      {picked.map((p) => <FriendRow key={p.root_id} person={p} />)}
    </Section>
  ) : (
    <Empty tab={tab} />
  );
}

// Requests can share a name, so the pending tab asks the person to check the tag before accepting.
function PendingTip() {
  return (
    <div className="flex items-center gap-s-2 rounded-lg bg-sidebar p-s-3 text-ui leading-5 text-muted-foreground">
      <Info className="size-5 shrink-0 text-accent-text" aria-hidden="true" />
      <span>Names can repeat. Check the tag after the dot matches the person you know before you accept.</span>
    </div>
  );
}

// The body of the page: the Add Friend form, or the search and the lists of the tab.
export function FriendsBody() {
  const data = useContacts();
  const tab = useFriendsView((s) => s.tab);
  const query = useFriendsView((s) => s.query);
  const addOpen = useFriendsView((s) => s.addOpen);
  const setQuery = useFriendsView((s) => s.setQuery);
  const current = activeTab(data, tab);

  if (addOpen) {
    return (
      <div className="min-h-0 overflow-y-auto px-s-6 py-s-4 dissent-scroll">
        <AddFriend />
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-col overflow-y-auto px-s-6 py-s-4 dissent-scroll">
      <Input
        type="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search"
        aria-label="Search friends"
        className="mb-s-4 h-10 w-full rounded-lg border-input-border bg-input"
      />
      <FriendLists data={data} tab={current} query={query} />
    </div>
  );
}

// Active Now: friends in a call first, then the rest who are online. A quiet note when nobody is.
export function FriendsAside() {
  const data = useContacts();
  const inCall = data.contacts.filter((p) => callOf(p.root_id));
  const online = data.contacts.filter((p) => isUp(p) && !callOf(p.root_id));
  const cards = [
    ...inCall.map((p) => ({ person: p, sub: `In a call · ${callOf(p.root_id)}` })),
    ...online.map((p) => ({ person: p, sub: PRESENCE_LABEL[presenceOfPerson(p)] })),
  ];
  return (
    <aside aria-label="Active now" className="flex min-h-0 flex-col gap-s-4 overflow-y-auto p-s-4 dissent-scroll">
      <h3 className="text-[17px] leading-6 font-bold text-text-strong">Active Now</h3>
      {cards.length ? (
        <div className="flex flex-col gap-s-1">
          {cards.map(({ person, sub }) => (
            <button
              key={person.root_id}
              type="button"
              onClick={() => void useShell.getState().openDm(person.root_id, person.display_name)}
              className="flex min-h-14 w-full items-center gap-s-3 rounded-lg bg-sidebar p-s-2 text-left hover:bg-elevated"
            >
              <UserAvatar name={person.display_name} seed={person.root_id} presence={presenceOfPerson(person)} />
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-ui font-semibold text-text-strong">{person.display_name}</span>
                <span className="truncate text-meta leading-4 text-muted-foreground">{sub}</span>
              </span>
            </button>
          ))}
        </div>
      ) : (
        <div className="rounded-lg bg-sidebar p-s-4 text-center">
          <div className="text-ui font-semibold text-text-strong">It’s quiet for now…</div>
          <div className="mt-s-1 text-meta leading-[18px] text-muted-foreground">When a friend joins a call or comes online, they’ll show up here.</div>
        </div>
      )}
    </aside>
  );
}
