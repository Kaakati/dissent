// Guardian recovery (docs/design/03-recovery-and-safety.md, "Recovering a lost key"): the recovering device's dialog,
// which shows the link and code to give each guardian and the answers as they come in, and the guardian's dialog,
// which approves only after the person read out the code on their screen. Nothing here signs or sends by itself:
// every step is a button the person presses.
import { useEffect, useState, type FormEvent } from 'react';
import { Check, Copy, KeyRound, ShieldAlert, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { UserAvatar } from '@/components/user-avatar';
import { QrCode } from '@/components/dialogs/qr-code';
import { ProgressBar } from '@/components/transfers/progress';
import { Callout } from '@/components/startup/gate';
import { api } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { confirm, openDialog } from '@/lib/dialog';
import { codeDigits, loadRecovery, progressText, useRecovery } from '@/lib/recovery';
import { toast } from '@/lib/toasts';
import type { RecoveryAnswerView, RecoveryGuardian, RecoveryRequestView, RecoveryView } from '@/lib/types';
import { cn } from '@/lib/utils';

const errorText = (err: unknown): string => (err instanceof Error ? err.message : String(err));

async function copy(text: string, what: string): Promise<void> {
  if (await copyText(text)) toast(`${what} copied`, 'info');
}

// ---- the recovering device ----

// Opens the recovery dialog: the start form, or the recovery already in progress. `start` fills the form, for a
// dissent://guardians/… link that was opened.
export function openRecoverDialog(start = ''): void {
  openDialog({
    title: 'Recover your account',
    sub: 'Your guardians vouch for the new key on this device. Nobody sends you the old key.',
    wide: true,
    content: (close) => <RecoverBody start={start} close={close} />,
  });
}

function RecoverBody({ start, close }: { start: string; close: () => void }) {
  const view = useRecovery((s) => s.view);
  const loaded = useRecovery((s) => s.loaded);
  useEffect(() => {
    void loadRecovery();
  }, []);
  if (!loaded) return <p role="status" className="text-muted-foreground">Loading…</p>;
  return view ? <Progress view={view} close={close} /> : <StartForm start={start} />;
}

function StartForm({ start }: { start: string }) {
  const [old, setOld] = useState(start);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!old.trim()) return;
    setBusy(true);
    setError('');
    try {
      useRecovery.getState().set(await api.beginRecovery(old.trim()));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="flex flex-col gap-s-4" onSubmit={(e) => void submit(e)} aria-label="Start recovery">
      <label className="flex flex-col gap-s-2 text-ui text-text">
        <span className="font-semibold text-text-strong">The account you lost</span>
        <Input
          autoFocus
          value={old}
          onChange={(e) => setOld(e.target.value)}
          placeholder="Its account id, or the dissent://guardians/… link from Settings › Recovery"
          aria-label="The account you lost"
        />
      </label>
      <p className="text-ui text-muted-foreground">
        A friend can tell you your account id: it is on your profile card. This device keeps the new key you just made, so save its recovery phrase too.
      </p>
      {error ? <p role="alert" className="text-ui text-danger-text">{error}</p> : null}
      <DialogFooter>
        <Button type="submit" disabled={!old.trim() || busy}>Start recovery</Button>
      </DialogFooter>
    </form>
  );
}

const STATE_LABEL: Record<RecoveryGuardian['state'], string> = { waiting: 'Waiting', approved: 'Approved', declined: 'Declined' };

function GuardianRow({ g }: { g: RecoveryGuardian }) {
  const name = g.display_name ?? `Guardian ·${g.suffix}`;
  return (
    <li className="flex min-h-12 items-center gap-s-3 rounded-lg px-s-2" data-guardian={g.root_id} data-state={g.state}>
      <UserAvatar name={name} seed={g.root_id} size="sm" />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-body text-text-strong">{name}</span>
        <span className="font-mono text-meta text-muted-foreground">·{g.suffix}</span>
      </span>
      <span
        className={cn(
          'flex items-center gap-1 rounded-full px-2.5 py-0.5 text-meta font-semibold',
          g.state === 'approved' && 'bg-success-tint text-success',
          g.state === 'declined' && 'bg-[rgb(208_16_90_/_0.16)] text-danger-text',
          g.state === 'waiting' && 'bg-rail text-muted-foreground',
        )}
      >
        {g.state === 'approved' ? <Check className="size-3.5" aria-hidden="true" /> : null}
        {g.state === 'declined' ? <X className="size-3.5" aria-hidden="true" /> : null}
        {STATE_LABEL[g.state]}
      </span>
    </li>
  );
}

function Progress({ view, close }: { view: RecoveryView; close: () => void }) {
  const [pasted, setPasted] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const add = async (e: FormEvent) => {
    e.preventDefault();
    if (!pasted.trim()) return;
    setError('');
    try {
      useRecovery.getState().set(await api.addRecoveryLink(pasted.trim()));
      setPasted('');
    } catch (err) {
      setError(errorText(err));
    }
  };
  const finish = async () => {
    setBusy(true);
    try {
      const servers = await api.finishRecovery();
      useRecovery.getState().set(null);
      toast(`Recovered. ${servers === 1 ? '1 server now knows' : `${servers} servers now know`} your new key.`, 'info');
      close();
    } catch (err) {
      toast(err);
    } finally {
      setBusy(false);
    }
  };
  const cancel = async () => {
    const ok = await confirm({
      title: 'Cancel this recovery?',
      message: 'Approvals you collected stop counting. You can start again, but your guardians must approve the new request.',
      okLabel: 'Cancel recovery',
      danger: true,
    });
    if (!ok) return;
    try {
      await api.cancelRecovery();
      useRecovery.getState().set(null);
    } catch (err) {
      toast(err);
    }
  };
  const fraction = view.threshold ? Math.min(1, view.approvals / view.threshold) : 0;

  return (
    <div className="flex flex-col gap-s-5" aria-label="Recovery in progress">
      <p className="text-body text-text">
        Recovering account <span className="font-mono text-text-strong">·{view.old_suffix}</span>.
      </p>

      <section className="grid grid-cols-[minmax(0,1fr)_auto] gap-s-4 rounded-lg border border-border bg-panel p-s-4">
        <div className="flex min-w-0 flex-col gap-s-3">
          <h3 className="text-ui font-semibold text-text-strong">1. Send this link to each guardian</h3>
          <div className="flex gap-s-2">
            <Input readOnly value={view.request_link} aria-label="Recovery link" className="font-mono text-meta" />
            <Button variant="outline" onClick={() => void copy(view.request_link, 'Recovery link')}>
              <Copy />
              Copy
            </Button>
          </div>
          <h3 className="mt-s-2 text-ui font-semibold text-text-strong">2. Call them, and read out this code</h3>
          <p aria-label="Recovery code" className="font-mono text-[28px] leading-9 font-bold tracking-[0.08em] text-text-strong tabular-nums">
            {view.code}
          </p>
          <Callout warn>
            Say the code out loud, on a call or face to face. Never send it with the link: hearing it from you is how a guardian knows it is really you.
          </Callout>
        </div>
        <QrCode text={view.request_link} label="Recovery link as a QR code" />
      </section>

      <section className="flex flex-col gap-s-2" aria-label="Guardian answers">
        <div className="flex items-center justify-between gap-s-2">
          <h3 className="text-ui font-semibold text-text-strong">3. Wait for their answers</h3>
          <span className="text-ui text-muted-foreground tabular-nums">{progressText(view)}</span>
        </div>
        {view.threshold !== null ? <ProgressBar value={fraction} /> : null}
        {view.guardians.length ? (
          <ul aria-label="Guardians" className="m-0 flex list-none flex-col gap-0.5 rounded-lg border border-border bg-panel p-s-2">
            {view.guardians.map((g) => <GuardianRow key={g.root_id} g={g} />)}
          </ul>
        ) : (
          <p className="text-ui text-muted-foreground">No answers yet. Each one shows up here as soon as a guardian approves or declines.</p>
        )}
        {view.invites ? (
          <p className="text-ui text-muted-foreground">{`${view.invites} server invite${view.invites === 1 ? '' : 's'} came with the answers, to rejoin when you finish.`}</p>
        ) : null}
        <form className="mt-s-2 flex gap-s-2" onSubmit={(e) => void add(e)}>
          <Input
            value={pasted}
            onChange={(e) => setPasted(e.target.value)}
            placeholder="A guardian sent you a link? Paste it here: dissent://approve/…, guardians/… or invite/…"
            aria-label="Paste a link from a guardian"
          />
          <Button type="submit" variant="outline" disabled={!pasted.trim()}>Add</Button>
        </form>
        {error ? <p role="alert" className="text-ui text-danger-text">{error}</p> : null}
      </section>

      <DialogFooter>
        <Button variant="outline" onClick={() => void cancel()}>Cancel recovery</Button>
        <Button variant="outline" onClick={close}>Close</Button>
        <Button disabled={!view.ready || busy} onClick={() => void finish()}>
          <KeyRound />
          Finish recovery
        </Button>
      </DialogFooter>
    </div>
  );
}

// ---- the guardian ----

// Opens the guardian's dialog for a dissent://recover/… link: who asks, the checks, then Approve or Decline.
export function openApproveRecoveryDialog(link: string): void {
  openDialog({
    title: 'A friend asks you to approve a recovery',
    sub: 'Approving lets their new key replace their account. Check it is really them first.',
    content: (close) => <ApproveBody link={link} close={close} />,
  });
}

function ApproveBody({ link, close }: { link: string; close: () => void }) {
  const [request, setRequest] = useState<RecoveryRequestView | null>(null);
  const [error, setError] = useState('');
  const [code, setCode] = useState('');
  const [heard, setHeard] = useState(false);
  const [invites, setInvites] = useState(true);
  const [busy, setBusy] = useState(false);
  const [answerError, setAnswerError] = useState('');
  const [result, setResult] = useState<{ approved: boolean; view: RecoveryAnswerView } | null>(null);

  useEffect(() => {
    let live = true;
    api.inspectRecoveryRequest(link).then(
      (r) => live && setRequest(r),
      (err: unknown) => live && setError(errorText(err)),
    );
    return () => {
      live = false;
    };
  }, [link]);

  if (error) {
    return (
      <>
        <p role="alert" className="text-ui text-danger-text">{error}</p>
        <DialogFooter>
          <Button variant="outline" onClick={close}>Close</Button>
        </DialogFooter>
      </>
    );
  }
  if (!request) return <p role="status" className="text-muted-foreground">Reading the request…</p>;

  const name = request.old_name ?? `Account ·${request.old_suffix}`;
  const answer = async (approve: boolean) => {
    setBusy(true);
    setAnswerError('');
    try {
      const view = await api.answerRecoveryRequest(link, approve, approve ? code : null, approve && invites);
      setResult({ approved: approve, view });
    } catch (err) {
      setAnswerError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (result) {
    const { approved, view } = result;
    return (
      <div className="flex flex-col gap-s-4" aria-label="Recovery answered">
        <p className="text-body text-text-strong">
          {approved ? `You approved ${name}'s new key.` : `You declined ${name}'s request.`}
        </p>
        <p className="text-ui text-muted-foreground">
          {view.delivered ? 'Their new device has your answer.' : view.note ?? 'Their new device could not be reached.'}
        </p>
        {!view.delivered && view.approval_link ? (
          <div className="flex gap-s-2">
            <Input readOnly value={view.approval_link} aria-label="Approval link" className="font-mono text-meta" />
            <Button variant="outline" onClick={() => void copy(view.approval_link ?? '', 'Approval link')}>
              <Copy />
              Copy
            </Button>
          </div>
        ) : null}
        <DialogFooter>
          <Button onClick={close}>Done</Button>
        </DialogFooter>
      </div>
    );
  }

  const codeReady = codeDigits(code).length === 9;
  return (
    <div className="flex flex-col gap-s-4" aria-label="Recovery request">
      <div className="flex items-center gap-s-3 rounded-lg border border-border bg-panel px-s-4 py-s-3">
        <UserAvatar name={name} seed={request.old_root} />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-body font-semibold text-text-strong">{name}</span>
          <span className="font-mono text-meta text-muted-foreground">
            ·{request.old_suffix} → new key ·{request.new_suffix}
          </span>
        </div>
      </div>
      {request.already_moved ? (
        <Callout warn>{`${name}'s account already moved to another key. Someone may have recovered it already, maybe not them. Decline unless they tell you on a call that this new request is theirs.`}</Callout>
      ) : request.key_changed_ms ? (
        <Callout warn>{`${name}'s key changed on ${new Date(request.key_changed_ms).toLocaleDateString()}. Compare safety numbers with them before you approve anything.`}</Callout>
      ) : null}
      {request.listed === true ? <p className="text-ui text-muted-foreground">You are one of their guardians.</p> : null}
      {request.listed === false ? (
        <Callout warn>You are not on their guardian list, so your approval will not count toward their recovery.</Callout>
      ) : null}
      {request.listed === null ? (
        <p className="text-ui text-muted-foreground">This device does not hold their guardian list. Your approval counts only if they named you.</p>
      ) : null}

      <div className="flex items-start gap-s-3 rounded-lg border border-border bg-panel px-s-4 py-s-3 text-ui text-text">
        <ShieldAlert className="mt-0.5 size-4 shrink-0 text-accent-text" aria-hidden="true" />
        <span>
          Call {name} or meet them. Someone who took over their account, or their messages, could send you this link. Approve only when you have heard from them yourself.
        </span>
      </div>

      <label className="flex flex-col gap-s-2 text-ui text-text">
        <span className="font-semibold text-text-strong">The code on their screen, as they read it to you</span>
        <Input
          inputMode="numeric"
          autoComplete="off"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          placeholder="123 456 789"
          aria-label="Code they read to you"
          className="font-mono tracking-[0.08em]"
        />
      </label>
      <label className="flex items-center gap-s-3 text-ui text-text">
        <input type="checkbox" checked={heard} onChange={(e) => setHeard(e.target.checked)} className="size-5 shrink-0 accent-accent" />
        <span>I heard this code from {name} directly, by voice or in person</span>
      </label>
      {request.shared_servers.length && request.reachable ? (
        <label className="flex items-center gap-s-3 text-ui text-text">
          <input type="checkbox" checked={invites} onChange={(e) => setInvites(e.target.checked)} className="size-5 shrink-0 accent-accent" />
          <span>{`Also send invites to the servers you share: ${request.shared_servers.join(', ')}`}</span>
        </label>
      ) : null}
      {answerError ? <p role="alert" className="text-ui text-danger-text">{answerError}</p> : null}
      <p className="text-ui text-muted-foreground">If anything feels off, or you cannot reach them, decline. They can ask again.</p>
      <DialogFooter>
        <Button variant="outline" disabled={busy || !codeReady || !heard} onClick={() => void answer(true)}>Approve</Button>
        <Button autoFocus disabled={busy} onClick={() => void answer(false)}>Decline</Button>
      </DialogFooter>
    </div>
  );
}

// Opens the right dialog for a link someone pasted on Settings › Recovery, or for a dissent:// link that was opened.
export function openRecoveryLink(link: string): void {
  const l = link.trim();
  if (/^dissent:\/\/recover\//i.test(l)) {
    openApproveRecoveryDialog(l);
  } else if (/^dissent:\/\/guardians\//i.test(l)) {
    void loadRecovery().then((view) => {
      if (view) {
        void api.addRecoveryLink(l).then((next) => useRecovery.getState().set(next), (err: unknown) => toast(err));
      }
      openRecoverDialog(view ? '' : l);
    });
  } else if (/^dissent:\/\/approve\//i.test(l)) {
    void api.addRecoveryLink(l).then(
      (next) => {
        useRecovery.getState().set(next);
        openRecoverDialog();
      },
      (err: unknown) => toast(err),
    );
  } else {
    toast('Paste a dissent://recover/… link a friend sent you.');
  }
}
