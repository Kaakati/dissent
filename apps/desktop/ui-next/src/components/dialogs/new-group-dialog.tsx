// Create a group: pick at least three contacts, see them as chips, name the group, then create it. The footer button
// says how many people the group will have, counting me. `preselect` people (the DM's person, from "Add friends to
// group") start picked and join the list when they are not a contact yet. Ported from openNewGroupDialog in
// ui/views/dialogs.js.
import { useMemo, useState } from 'react';
import { X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { UserAvatar } from '@/components/user-avatar';
import { matchesSearch, shortOf, addressOf } from '@/lib/address';
import { createGroup } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import type { Person } from '@/lib/types';
import { cn } from '@/lib/utils';

const MIN_PEOPLE = 3;

function GroupBody({ preselect, close }: { preselect: Person[]; close: () => void }) {
  const contacts = useShell((s) => s.contacts?.contacts ?? []);
  // Contacts first, then the preselected people who are not contacts yet.
  const people = useMemo(
    () => [...contacts, ...preselect.filter((p) => !contacts.some((c) => c.root_id === p.root_id))],
    [contacts, preselect],
  );
  const [picked, setPicked] = useState<string[]>(() => preselect.map((p) => p.root_id));
  const [name, setName] = useState('New group');
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);

  const n = picked.length;
  const ok = n >= MIN_PEOPLE;
  const shown = people.filter((p) => matchesSearch(p, query.trim().toLowerCase()));
  const toggle = (id: string, on: boolean) => setPicked((list) => (on ? [...list, id] : list.filter((x) => x !== id)));

  const create = async () => {
    setBusy(true);
    const done = await createGroup(name.trim() || 'New group', picked);
    setBusy(false);
    if (done) close();
  };

  return (
    <>
      <p className="text-sm text-muted-foreground">Pick at least 3 people. Only the people in the group can read it.</p>
      <div className="flex flex-wrap items-center gap-s-2 rounded-lg border border-input-border bg-input p-s-2 focus-within:shadow-[0_0_0_2px_var(--accent)]">
        {people
          .filter((p) => picked.includes(p.root_id))
          .map((p) => (
            <button
              key={p.root_id}
              type="button"
              aria-label={`Remove ${p.display_name}`}
              onClick={() => toggle(p.root_id, false)}
              className="inline-flex h-7 items-center gap-1.5 rounded-pill bg-elevated py-0 pr-1.5 pl-1 text-ui font-medium text-text-strong"
            >
              <UserAvatar name={p.display_name} seed={p.root_id} size="xs" className="size-5 text-[7px]" />
              <span>{p.display_name}</span>
              <X className="size-3.5 text-muted-foreground" aria-hidden="true" />
            </button>
          ))}
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Type a name or paste an address"
          aria-label="Find contacts"
          className="h-8 min-w-40 flex-1 bg-transparent px-1 text-ui text-text-strong placeholder:text-muted-foreground focus-visible:outline-none"
        />
      </div>
      <div className="flex items-center justify-between">
        <span className="text-sm font-semibold tracking-[.02em] text-muted-foreground uppercase">Contacts</span>
        <span className={cn('text-ui font-medium text-muted-foreground tabular-nums', !ok && 'text-warn')}>
          {ok ? `${n} selected · group of ${n + 1} with you` : `Pick ${MIN_PEOPLE - n} more`}
        </span>
      </div>
      <ul aria-label="Contacts" className="dissent-scroll flex max-h-[392px] flex-col gap-0.5 overflow-y-auto">
        {shown.map((p) => {
          const on = picked.includes(p.root_id);
          return (
            <li key={p.root_id}>
              <label className={cn('flex h-12 cursor-pointer items-center gap-3 rounded-md px-2 hover:bg-hover', on && 'bg-hover')}>
                <UserAvatar name={p.display_name} seed={p.root_id} />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-ui font-medium text-text-strong">{p.display_name}</span>
                  <span className="font-mono text-meta text-muted-foreground" title={addressOf('user', p.root_id)}>{`·${shortOf('user', p.root_id)}`}</span>
                </span>
                <input type="checkbox" className="size-5 m-0 accent-accent" checked={on} onChange={(e) => toggle(p.root_id, e.target.checked)} />
              </label>
            </li>
          );
        })}
        {shown.length === 0 ? (
          <li className="p-s-2 text-sm text-muted-foreground">{people.length ? 'No contacts match.' : 'Add a contact first.'}</li>
        ) : null}
      </ul>
      <label className="flex flex-col gap-1">
        <span className="text-sm font-semibold tracking-[.02em] text-muted-foreground uppercase">Group name</span>
        <Input value={name} aria-label="Group name" onChange={(e) => setName(e.target.value)} />
      </label>
      <DialogFooter>
        <Button variant="outline" onClick={close}>Cancel</Button>
        <Button onClick={() => void create()} disabled={!ok || busy}>
          {ok ? `Create group of ${n + 1}` : 'Create group'}
        </Button>
      </DialogFooter>
    </>
  );
}

export function openNewGroupDialog({ preselect = [] }: { preselect?: Person[] } = {}): void {
  openDialog({
    title: 'Create a group',
    className: 'w-[min(520px,92vw)]',
    content: (close) => <GroupBody preselect={preselect} close={close} />,
  });
}
