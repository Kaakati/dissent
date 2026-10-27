// First run: the identity screen, shown once when the engine reports `me.first_run`. It covers the app below the title
// bar until the user confirms they wrote the recovery phrase down. The engine clears the flag; the screen only hides.
// Ported from views/onboarding.js.
import { useEffect, useState } from 'react';
import { Copy } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { Callout } from '@/components/startup/gate';
import { OwlMark } from '@/components/owl-mark';
import { UserAvatar } from '@/components/user-avatar';
import { openDialog } from '@/lib/dialog';
import { copyText } from '@/lib/clipboard';
import { api } from '@/lib/api';
import { shortOf } from '@/lib/address';
import { toast } from '@/lib/toasts';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';

// Restore instead of creating a new identity: the same flow as Settings > Import phrase.
function openRestore(onRestored: () => void) {
  openDialog({
    title: 'Restore from a recovery phrase',
    sub: 'Replaces the identity on this device with the one from the phrase.',
    content: (close) => <RestoreForm onDone={(restored) => { if (restored) { close(); onRestored(); } }} onCancel={close} />,
  });
}

function RestoreForm({ onDone, onCancel }: { onDone: (restored: boolean) => void; onCancel: () => void }) {
  const [phrase, setPhrase] = useState('');
  const restoreIdentity = useShell((s) => s.restoreIdentity);
  const restore = async () => {
    const words = phrase.trim();
    if (!words) return;
    onDone(await restoreIdentity(words));
  };
  return (
    <div className="flex flex-col gap-s-4">
      <Textarea
        rows={3}
        placeholder="24 recovery words, separated by spaces"
        aria-label="Recovery phrase to restore"
        value={phrase}
        onChange={(e) => setPhrase(e.target.value)}
      />
      <div className="-mx-s-5 -mb-s-5 flex justify-end gap-s-2 bg-sidebar px-s-5 py-s-4">
        <Button variant="outline" onClick={onCancel}>
          Cancel
        </Button>
        <Button onClick={() => void restore()}>Restore</Button>
      </div>
    </div>
  );
}

// One step of the list on the left. Done steps have a green dot, the current one is highlighted.
function Step({ state, n, children }: { state: 'done' | 'current' | 'todo'; n: number; children: string }) {
  return (
    <li
      aria-current={state === 'current' ? 'step' : undefined}
      className={cn(
        'flex h-12 items-center gap-3.5 rounded-lg px-3 text-body font-medium text-muted-foreground',
        state === 'current' && 'bg-selected font-semibold text-text-strong',
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          'grid size-7 place-items-center rounded-full text-xs font-bold text-white',
          state === 'done' && 'bg-success',
          state === 'current' && 'bg-accent',
          state === 'todo' && 'shadow-[inset_0_0_0_2px_var(--faint)] text-muted-foreground',
        )}
      >
        {state === 'todo' ? n : state === 'current' ? n : null}
      </span>
      {children}
    </li>
  );
}

export function Onboarding() {
  const open = useShell((s) => s.onboarding);
  return open ? <FirstRun /> : null;
}

// Mounted while the screen is open, so each showing starts unticked and reads the phrase afresh.
function FirstRun() {
  const me = useShell((s) => s.me);
  const finishFirstRun = useShell((s) => s.finishFirstRun);
  const [words, setWords] = useState<string[]>([]);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  // The phrase is read once, when the screen opens.
  useEffect(() => {
    let cancelled = false;
    api
      .identityExportPhrase()
      .then((phrase) => {
        if (!cancelled) setWords(String(phrase).trim().split(/\s+/).filter(Boolean));
      })
      .catch((err) => toast(err));
    return () => {
      cancelled = true;
    };
  }, []);

  const copy = async () => {
    if (await copyText(words.join(' '))) toast('Recovery phrase copied', 'info');
  };
  const cont = async () => {
    setBusy(true);
    await finishFirstRun();
    setBusy(false);
  };

  return (
    <div role="dialog" aria-modal="true" aria-label="Welcome to Dissent" className="fixed inset-x-0 top-[30px] bottom-0 z-[80] grid grid-cols-[640px_minmax(0,1fr)] gap-s-2 bg-rail p-s-2">
      <section className="flex flex-col gap-s-6 overflow-hidden rounded-lg border border-border bg-sidebar px-[72px] py-20">
        <div className="flex items-center gap-3.5 text-[27px] leading-[34px] font-bold text-text-strong">
          <span className="grid size-14 place-items-center rounded-2xl bg-accent text-white">
            <OwlMark className="size-8" />
          </span>
          <span>Dissent</span>
        </div>
        <h1 className="mt-s-10 text-[51px] leading-[60px] font-bold tracking-[-0.01em] text-text-strong">You are your key, not an account.</h1>
        <p className="max-w-[440px] text-[17px] leading-7 text-muted-foreground">
          No sign-up, email or phone number. Your identity was just created on this device, and no company keeps a list of users.
        </p>
        <ol className="m-0 mt-auto flex list-none flex-col gap-1.5 p-0">
          <Step state="done" n={1}>Pick a display name</Step>
          <Step state="current" n={2}>Save your recovery phrase</Step>
          <Step state="todo" n={3}>Join a server or add a contact</Step>
        </ol>
      </section>

      <section className="flex flex-col items-center justify-center gap-s-6 overflow-y-auto rounded-lg border border-border bg-chat py-[60px]">
        <div className="flex w-[960px] max-w-full items-center gap-s-4 rounded-lg border border-border bg-panel px-s-5 py-s-4">
          <UserAvatar name={me?.display_name || '?'} seed={me?.root_id} />
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="text-[17px] leading-6 font-semibold text-text-strong">{me?.display_name || 'Set a display name later'}</span>
            <span className="font-mono text-ui text-muted-foreground">{me?.root_id ? `·${shortOf('user', me.root_id)}` : ''}</span>
          </div>
          <p className="max-w-[380px] text-ui text-muted-foreground">Names can repeat. The tag after the dot comes from your key, so people can tell you apart.</p>
        </div>

        <div className="flex w-[960px] max-w-full flex-col gap-s-6">
          <h2 className="text-[29px] leading-[38px] font-bold text-text-strong">Your recovery phrase</h2>
          <p className="text-body text-muted-foreground">
            With these {words.length} words you can restore yourself on a new computer. Nobody else holds a copy, so nobody can recover it for you.
          </p>
          <ol aria-label="Recovery phrase" className="m-0 grid list-none grid-cols-4 gap-2.5 rounded-lg bg-ink p-s-4 font-mono text-body tabular-nums">
            {words.map((w, i) => (
              <li key={`${i}-${w}`} className="flex h-13 items-center gap-3 rounded-lg bg-sidebar px-4">
                <span className="w-[22px] text-right text-xs text-faint">{i + 1}</span>
                <span className="text-text-strong">{w}</span>
              </li>
            ))}
          </ol>
          <Callout warn>Anyone with these words controls your identity. Keep them offline and never share them.</Callout>
          <div className="mt-auto flex flex-wrap items-center gap-s-3">
            <label className="flex min-w-[200px] flex-1 items-center gap-s-3 text-ui text-text">
              <input
                type="checkbox"
                id="onboard-saved"
                autoFocus
                checked={saved}
                onChange={(e) => setSaved(e.target.checked)}
                className="size-5 shrink-0 accent-accent"
              />
              <span>I wrote these words down somewhere safe</span>
            </label>
            <Button variant="outline" className="h-11" onClick={() => void copy()}>
              <Copy />
              Copy
            </Button>
            <Button className="h-11 px-6" disabled={!saved || busy} onClick={() => void cont()}>
              Continue
            </Button>
          </div>
          <p className="text-body text-muted-foreground">
            Already have an identity?{' '}
            <button
              type="button"
              className="text-accent-text hover:underline"
              onClick={() => openRestore(() => useShell.setState({ onboarding: false, titleOverride: null }))}
            >
              Restore from a recovery phrase
            </button>
          </p>
        </div>
      </section>
    </div>
  );
}
