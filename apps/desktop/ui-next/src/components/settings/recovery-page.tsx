// Settings > Recovery: the guardians who can vouch for a new key, and how many of them must agree. Saving signs the list.
// Cancel drops unsaved edits and closes Settings, as the current UI does. Ported from views/guardians.js.
import { useEffect, useState } from 'react';
import { KeyRound, LifeBuoy, UserPlus, X } from 'lucide-react';
import { openRecoverDialog, openRecoveryLink } from '@/components/recovery/recovery-dialogs';
import { Input } from '@/components/ui/input';
import { Block } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { UserAvatar } from '@/components/user-avatar';
import { api } from '@/lib/api';
import { shortOf } from '@/lib/address';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { GuardianView, GuardianList } from '@/lib/types';
import { cn } from '@/lib/utils';

const MAX_GUARDIANS = 16;
const WORD = ['Zero', 'One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight', 'Nine', 'Ten', 'Eleven', 'Twelve', 'Thirteen', 'Fourteen', 'Fifteen', 'Sixteen'];

// A contact who can be picked as a guardian.
interface Candidate {
  root_id: string;
  display_name: string;
  suffix?: string;
}

const plural = (n: number) => `${n} shared server${n === 1 ? '' : 's'}`;
// The engine names the shared servers; a count is taken as it is.
const countOf = (shared: number | string[]) => (Array.isArray(shared) ? shared.length : shared);

export function RecoveryPage() {
  const closeSettings = useShell((s) => s.closeSettings);
  const [error, setError] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [guardians, setGuardians] = useState<GuardianView[]>([]);
  const [threshold, setThreshold] = useState(1);
  // The last saved state. Cancel drops unsaved edits back to it.
  const [saved, setSaved] = useState<GuardianList>({ threshold: 1, guardians: [] });
  const [pickerOpen, setPickerOpen] = useState(false);
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [pickerError, setPickerError] = useState('');
  const [friendLink, setFriendLink] = useState('');

  useEffect(() => {
    api
      .listGuardians()
      .then((view) => {
        setGuardians(view.guardians.map((g) => ({ ...g })));
        setThreshold(view.threshold);
        setSaved({ threshold: view.threshold, guardians: view.guardians.map((g) => ({ ...g })) });
        setLoaded(true);
      })
      .catch((err) => setError(`Could not load guardians: ${err?.message ?? err}`));
  }, []);

  // Contacts that are not guardians yet. Picking one adds it, up to the maximum.
  const togglePicker = async () => {
    const open = !pickerOpen;
    setPickerOpen(open);
    if (!open) return;
    setPickerError('');
    try {
      setCandidates(((await api.listContacts()).contacts as Candidate[]) ?? []);
    } catch (err) {
      setPickerError(`Could not load contacts: ${(err as Error)?.message ?? err}`);
    }
  };

  const chosen = new Set(guardians.map((g) => g.root_id));
  const free = candidates.filter((c) => !chosen.has(c.root_id));
  const pick = (c: Candidate) => {
    if (guardians.length >= MAX_GUARDIANS) {
      toast(`You can have up to ${MAX_GUARDIANS} guardians`);
      return;
    }
    setGuardians([...guardians, { root_id: c.root_id, display_name: c.display_name, suffix: c.suffix, shared_servers: 0 }]);
    setPickerOpen(false);
  };
  const remove = (id: string) => {
    const next = guardians.filter((g) => g.root_id !== id);
    setGuardians(next);
    setThreshold(Math.min(threshold, Math.max(1, next.length)));
  };

  const cancel = () => {
    setGuardians(saved.guardians.map((g) => ({ ...g })));
    setThreshold(saved.threshold);
    setPickerOpen(false);
    closeSettings();
  };

  const save = async () => {
    try {
      const signed = await api.signGuardians(guardians.map((g) => g.root_id), threshold);
      setGuardians(signed.guardians.map((g) => ({ ...g })));
      setThreshold(signed.threshold);
      setSaved({ threshold: signed.threshold, guardians: signed.guardians.map((g) => ({ ...g })) });
      toast('Guardians saved', 'info');
    } catch (err) {
      toast(err);
    }
  };

  if (error) return <p className="text-danger-text">{error}</p>;
  if (!loaded) return <p className="text-muted-foreground">Loading…</p>;

  const count = Math.max(1, guardians.length);
  const hint = threshold === 1
    ? 'Any one guardian can move your identity. Fast, but a single compromised friend is enough.'
    : `${WORD[threshold]} of your ${WORD[guardians.length].toLowerCase()} guardians must approve. One lost or unreachable guardian won’t lock you out.`;

  return (
    <div className="flex min-h-full max-w-[740px] flex-col gap-s-3">
      <h2 className="mb-s-2 text-head font-bold text-text-strong">Recovery</h2>
      <p className="text-muted-foreground">
        If you lose your recovery phrase, enough of your guardians can vouch for a new key. Your roles and handles in every server move with you. Guardians never hold a copy of your key.
      </p>
      <div className="flex items-center justify-between gap-s-2">
        <h3 className="mt-s-2 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">
          Guardians — {guardians.length} of {MAX_GUARDIANS}
        </h3>
        <Button variant="outline" onClick={() => void togglePicker()}>
          <UserPlus />
          Add guardian
        </Button>
      </div>

      {pickerOpen ? (
        <div className="flex flex-wrap gap-1.5 rounded-lg bg-sidebar p-s-2">
          {pickerError ? <p className="text-danger-text">{pickerError}</p> : null}
          {!pickerError && free.length === 0 ? <p className="text-muted-foreground">Every contact is already a guardian.</p> : null}
          {free.map((c) => (
            <Button key={c.root_id} variant="outline" onClick={() => pick(c)}>
              {c.display_name}
            </Button>
          ))}
        </div>
      ) : null}

      <ul aria-label="Guardians" className="m-0 flex list-none flex-col gap-0.5 rounded-lg border border-border bg-panel p-s-2">
        {guardians.map((g) => (
          <li key={g.root_id} className="flex min-h-14 items-center gap-s-3 rounded-lg px-s-2">
            <UserAvatar name={g.display_name} seed={g.root_id} size="md" />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-body text-text-strong">{g.display_name}</span>
              <span className="font-mono text-meta text-muted-foreground">·{shortOf('user', g.root_id)}</span>
            </span>
            <span className="text-ui text-muted-foreground">{countOf(g.shared_servers) ? plural(countOf(g.shared_servers)) : 'Contact'}</span>
            <Button variant="ghost" size="icon" aria-label={`Remove ${g.display_name}`} onClick={() => remove(g.root_id)}>
              <X />
            </Button>
          </li>
        ))}
      </ul>

      <Block title="How many must agree" className="mt-s-2">
        <div role="radiogroup" aria-label="How many must agree" className="flex gap-s-2">
          {Array.from({ length: count }, (_, i) => i + 1).map((n) => (
            <button
              key={n}
              type="button"
              role="radio"
              aria-checked={n === threshold}
              onClick={() => setThreshold(n)}
              className={cn(
                'h-11 min-w-28 rounded-lg border border-border-strong bg-transparent px-s-4 text-body font-semibold text-text-strong tabular-nums',
                n === threshold && 'border-accent-text bg-[rgb(255_79_0_/_0.18)]',
              )}
            >
              {n} of {count}
            </button>
          ))}
        </div>
        <p className="text-muted-foreground">{hint}</p>
      </Block>

      <div className="flex items-start gap-s-3 rounded-lg border border-border bg-panel px-s-4 py-3.5 text-ui text-muted-foreground">
        <KeyRound className="mt-0.5 size-4 shrink-0 text-accent-text" aria-hidden="true" />
        <span>
          Saving signs this list with your key and shares it with your devices. To recover, open Recover account on a new device and send the link it shows to your guardians.
        </span>
      </div>

      <Block title="Help a friend recover" className="mt-s-2">
        <p className="text-muted-foreground">
          If you are someone&apos;s guardian and they sent you a recovery link, open it here. You approve only after they read you the code on their screen.
        </p>
        <form
          className="flex gap-s-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (friendLink.trim()) openRecoveryLink(friendLink);
          }}
        >
          <Input value={friendLink} onChange={(e) => setFriendLink(e.target.value)} placeholder="dissent://recover/…" aria-label="A friend's recovery link" />
          <Button type="submit" variant="outline" disabled={!friendLink.trim()}>Open</Button>
        </form>
      </Block>

      <Block title="Recover a lost account" className="mt-s-2">
        <p className="text-muted-foreground">
          Lost the device that held your account? On this new device, ask your guardians to approve this device&apos;s key in its place.
        </p>
        <div>
          <Button variant="outline" onClick={() => openRecoverDialog()}>
            <LifeBuoy />
            Recover with guardians
          </Button>
        </div>
      </Block>

      <div className="mt-auto flex justify-end gap-s-2 pt-s-2">
        <Button variant="outline" onClick={cancel}>
          Cancel
        </Button>
        <Button disabled={guardians.length === 0 || threshold < 1 || threshold > guardians.length} onClick={() => void save()}>
          Sign and save
        </Button>
      </div>
    </div>
  );
}
