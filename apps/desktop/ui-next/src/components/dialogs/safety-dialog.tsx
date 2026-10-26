// The safety number: the same 12 groups of five digits on both sides. Compare it in person or on a call. When the
// peer's key has changed, an amber banner comes first, and "They match" marks the number compared. Ported from
// openSafetyDialog in the old UI's views/dialogs.js.
import { CircleAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { has } from '@/lib/features';
import { markSafetyVerified, safetyNumber } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { toast } from '@/lib/toasts';

export interface SafetyPerson {
  name: string;
  // When the peer's key changed (key_change feature), in epoch milliseconds. Unset when it did not.
  keyChangedMs?: number | null;
}

function SafetyBody({ userId, number, person, close }: { userId: string; number: string; person: SafetyPerson; close: () => void }) {
  const { name, keyChangedMs } = person;
  const changed = has('key_change') && Boolean(keyChangedMs);
  const groups = number.split(' ');
  const when = keyChangedMs
    ? new Date(Number(keyChangedMs)).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
    : '';

  const match = async () => {
    if (await markSafetyVerified(userId)) {
      toast('Safety number marked as compared', 'info');
      close();
    }
  };

  return (
    <>
      {changed ? (
        <div role="status" className="flex items-start gap-s-2 rounded-lg bg-warn-tint p-s-3 text-sm leading-[19px] text-warn">
          <CircleAlert className="mt-px size-4 shrink-0" aria-hidden="true" />
          <span>{`${name}'s key changed on ${when}. They may have recovered their account. Compare before you trust new messages.`}</span>
        </div>
      ) : null}
      <p className="text-sm leading-[19px] text-muted-foreground">
        {changed
          ? `Compare these numbers with ${name} in person or on a call. If they match on both screens, nobody has swapped in a different key for either of you.`
          : `Compare these numbers with ${name} in person or on a call. If they match on both screens, your chat is verified.`}
      </p>
      <ol aria-label="Safety number" className="grid grid-cols-4 gap-s-2 font-mono text-body text-text-strong tabular-nums">
        {groups.map((g, i) => (
          <li key={i} className="grid h-11 place-items-center rounded-lg bg-rail">
            {g}
          </li>
        ))}
      </ol>
      <DialogFooter>
        <Button variant="outline" onClick={close}>Not now</Button>
        {has('safety') ? <Button onClick={() => void match()}>They match</Button> : null}
      </DialogFooter>
    </>
  );
}

// Loads the number first; nothing opens when it cannot be read (the failure is a toast).
export async function openSafetyDialog(userId: string, person: SafetyPerson): Promise<void> {
  const number = await safetyNumber(userId);
  if (!number) return;
  const changed = has('key_change') && Boolean(person.keyChangedMs);
  openDialog({
    title: changed ? `Safety number with ${person.name}` : 'Verify safety number',
    content: (close) => <SafetyBody userId={userId} number={number} person={person} close={close} />,
  });
}
