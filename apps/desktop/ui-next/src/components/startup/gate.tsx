// The screens that run before anything else starts, for a portable build: the four-option chooser on a first run, the
// passphrase screens for a locked profile, and the link screen for a new device. Nothing opens until one of them
// finishes, and the overlay cannot be dismissed. Ported from views/unlock.js.
import { useState, type FormEvent, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { CircleAlert } from 'lucide-react';
import { OwlMark } from '@/components/owl-mark';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { MIN_PASSPHRASE, strengthHint } from '@/lib/passphrase';
import { toast } from '@/lib/toasts';
import type { VaultStatus } from '@/lib/types';
import { cn } from '@/lib/utils';

type Choice = 'installed' | 'link' | 'copy' | 'fresh';

const OPTIONS: { choice: Choice; title: string; sentence: string; needsInstalled?: boolean }[] = [
  {
    choice: 'installed',
    title: "Use this computer's Dissent profile",
    sentence: 'Opens the profile the installed Dissent app already uses here. Nothing is copied.',
    needsInstalled: true,
  },
  {
    choice: 'link',
    title: 'Link as a new device (recommended for a USB stick)',
    sentence: 'Gives this stick its own device key, which your installed Dissent approves and can remove later.',
  },
  {
    choice: 'copy',
    title: 'Copy my profile into this portable folder',
    sentence: 'Makes a passphrase-protected copy of the installed profile. Both copies then share one device identity, so linking is safer.',
    needsInstalled: true,
  },
  {
    choice: 'fresh',
    title: 'Start fresh / restore from recovery phrase',
    sentence: 'Starts a new identity protected by a passphrase, or restores one from your recovery phrase.',
  },
];

// The brand line at the top of each card.
function Brand() {
  return (
    <div className="flex items-center gap-3.5 text-[27px] leading-[34px] font-bold text-text-strong">
      <span className="grid size-14 place-items-center rounded-2xl bg-accent text-white">
        <OwlMark className="size-8" />
      </span>
      <span>Dissent</span>
    </div>
  );
}

// The card on the overlay. A form takes the submit handler; the chooser is a plain section.
function Card({ label, children, onSubmit }: { label: string; children: ReactNode; onSubmit?: (e: FormEvent) => void }) {
  const className = 'flex w-[min(420px,100%)] flex-col gap-s-3 rounded-lg border border-border bg-sidebar p-s-8';
  return (
    <div role="dialog" aria-modal="true" aria-label={label} className="fixed inset-0 z-[90] grid place-items-center overflow-auto bg-rail p-s-4">
      {onSubmit ? (
        <form className={className} onSubmit={onSubmit}>
          {children}
        </form>
      ) : (
        <section className={className}>{children}</section>
      )}
    </div>
  );
}

// A warning or plain note with an icon: the current UI's .callout.
export function Callout({ warn = false, children }: { warn?: boolean; children: ReactNode }) {
  return (
    <div
      className={cn(
        'flex items-start gap-s-3 rounded-lg border border-border bg-panel px-s-4 py-3.5 text-ui text-muted-foreground',
        warn && 'text-warn',
      )}
    >
      <CircleAlert className={cn('mt-0.5 size-4 shrink-0', warn ? 'text-warn' : 'text-accent-text')} aria-hidden="true" />
      <span>{children}</span>
    </div>
  );
}

function Chooser({ status }: { status: VaultStatus }) {
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const pick = async (choice: Choice) => {
    setError('');
    setBusy(true);
    try {
      // On success the app restarts in the chosen mode, so this call does not return to the page.
      await api.chooseProfile(choice);
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  };
  return (
    <Card label="How to use Dissent here">
      <Brand />
      <h1 className="text-title font-bold text-text-strong">How do you want to use Dissent here?</h1>
      <p className="text-muted-foreground">This choice is saved next to Dissent.exe. You can run this exe on another computer and choose again there.</p>
      {OPTIONS.map((option, i) => {
        const missing = Boolean(option.needsInstalled && !status.installed_profile);
        return (
          <button
            key={option.choice}
            type="button"
            data-choice={option.choice}
            autoFocus={i === 0}
            disabled={busy || missing}
            onClick={() => void pick(option.choice)}
            className="grid w-full cursor-pointer gap-s-1 rounded-lg border border-border-strong bg-sidebar p-s-4 text-left hover:bg-hover disabled:cursor-default disabled:opacity-55 disabled:hover:bg-sidebar"
          >
            <span className="text-ui font-semibold text-text-strong">{option.title}</span>
            <span className="text-meta leading-4 text-muted-foreground">
              {missing ? 'No Dissent profile was found on this computer, so this option is not available.' : option.sentence}
            </span>
          </button>
        );
      })}
      <p role="alert" className="text-danger-text">{error}</p>
    </Card>
  );
}

function Unlock({ status, onUnlocked }: { status: VaultStatus; onUnlocked: (next: 'link' | 'open') => Promise<void> }) {
  const creating = Boolean(status.new_profile);
  const copying = creating && status.choice === 'copy';
  const [pass, setPass] = useState('');
  const [again, setAgain] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const [submitting, setSubmitting] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError('');
    if (!pass) return;
    if (creating) {
      if (strengthHint(pass).level === 'short') {
        setError(`Use at least ${MIN_PASSPHRASE} characters.`);
        return;
      }
      if (pass !== again) {
        setError('The two passphrases don’t match.');
        return;
      }
    }
    setSubmitting(true);
    if (copying) setBusy('Copying your profile. This can take a minute. Dissent starts when it is done.');
    try {
      await onUnlocked(await api.unlockProfile(pass));
    } catch (err) {
      setBusy('');
      setError(String(err));
      setSubmitting(false);
    }
  };

  const title = copying ? 'Copy your profile' : creating ? 'Create a passphrase' : 'Unlock Dissent';
  return (
    <Card label="Unlock Dissent" onSubmit={submit}>
      <Brand />
      <h1 className="text-title font-bold text-text-strong">{title}</h1>
      <p className="text-muted-foreground">
        {copying
          ? 'This copies the profile of the installed Dissent into the DissentData folder next to Dissent.exe, under a new passphrase. Close the installed Dissent first.'
          : creating
            ? 'This portable profile is locked with a passphrase. Its data is kept in the DissentData folder next to Dissent.exe.'
            : 'Enter the passphrase for this portable profile.'}
      </p>
      <Input
        type="password"
        autoComplete={creating ? 'new-password' : 'current-password'}
        aria-label="Passphrase"
        autoFocus
        value={pass}
        onChange={(e) => setPass(e.target.value)}
      />
      {creating ? <p role="status" className="text-muted-foreground">{strengthHint(pass).text}</p> : null}
      {creating ? (
        <Input type="password" autoComplete="new-password" aria-label="Repeat the passphrase" value={again} onChange={(e) => setAgain(e.target.value)} />
      ) : null}
      {creating ? <Callout warn>Keep this safe. Without it this portable profile can’t be opened.</Callout> : null}
      {copying ? <Callout warn>Both copies now share one device identity. Linking a new device is safer for a USB stick.</Callout> : null}
      {/* The window's own storage is this PC's WebView2 profile, not DissentData. It holds no messages, but it does hold
          which servers, channels and conversations you have (read markers and mutes keyed by their ids) and layout choices. */}
      <Callout>
        On this PC, the Dissent window also keeps a few settings in its own storage, outside DissentData: which conversations
        and channels you have read, mutes, and layout choices. They stay on this PC after you unplug the stick. Your
        messages and contacts are only in DissentData, encrypted under the passphrase.
      </Callout>
      <p role="alert" className="text-danger-text">{error}</p>
      <p role="status" className="text-muted-foreground">{busy}</p>
      <div className="mt-s-2 flex justify-end">
        <Button type="submit" disabled={submitting}>
          {copying ? 'Copy profile' : creating ? 'Create profile' : 'Unlock'}
        </Button>
      </div>
    </Card>
  );
}

// The new device's link code, to approve from the installed app. Finishing stores the grant and opens the profile.
function Link({ code, onDone }: { code: string; onDone: () => void }) {
  const [grant, setGrant] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const finish = async (e: FormEvent) => {
    e.preventDefault();
    const value = grant.trim();
    if (!value) return;
    setError('');
    setSubmitting(true);
    try {
      await api.finishPortableLink(value);
      onDone();
    } catch (err) {
      setError(String(err));
      setSubmitting(false);
    }
  };
  return (
    <Card label="Link this device" onSubmit={finish}>
      <h1 className="text-title font-bold text-text-strong">Link this device</h1>
      <p className="text-muted-foreground">
        On the computer with your installed Dissent, open Settings, then Devices. Under "Approve a new device", paste this code and press Approve. It gives you a grant.
      </p>
      <div className="flex items-center gap-s-2">
        <Input readOnly value={code} aria-label="Link code for the installed Dissent" className="font-mono text-meta" />
        <Button
          variant="outline"
          onClick={async () => {
            if (await copyText(code)) toast('Link code copied', 'info');
          }}
        >
          Copy code
        </Button>
      </div>
      <p className="text-muted-foreground">
        Paste the grant it gives you below. This device then gets its own key, and you can remove it from the Devices list at any time.
      </p>
      <Input placeholder="Paste the grant here" aria-label="Grant from the installed Dissent" value={grant} onChange={(e) => setGrant(e.target.value)} />
      <p role="alert" className="text-danger-text">{error}</p>
      <div className="mt-s-2 flex justify-end">
        <Button type="submit" disabled={submitting}>
          Finish linking
        </Button>
      </div>
    </Card>
  );
}

function Gate({ status, onDone }: { status: VaultStatus; onDone: () => void }) {
  const [step, setStep] = useState<{ kind: 'chooser' } | { kind: 'unlock' } | { kind: 'link'; code: string }>(
    status.chooser ? { kind: 'chooser' } : { kind: 'unlock' },
  );
  if (step.kind === 'chooser') return <Chooser status={status} />;
  if (step.kind === 'link') return <Link code={step.code} onDone={onDone} />;
  return (
    <Unlock
      status={status}
      onUnlocked={async (next) => {
        if (next !== 'link') {
          onDone();
          return;
        }
        // A linked device shows its code first; the unlock card is replaced by the link card.
        setStep({ kind: 'link', code: await api.linkDeviceCode() });
      }}
    />
  );
}

// Shows the gate for a portable profile that is not open yet, and resolves once it is. A chooser never resolves here:
// the app restarts in the chosen mode.
export function runGate(status: VaultStatus): Promise<void> {
  return new Promise((done) => {
    const host = document.createElement('div');
    document.body.append(host);
    const root = createRoot(host);
    root.render(
      <Gate
        status={status}
        onDone={() => {
          root.unmount();
          host.remove();
          done();
        }}
      />,
    );
  });
}
