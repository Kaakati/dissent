// The incoming call (the old UI's views/incoming.js): an alert over the current view for a ringing direct or group call. It stays
// until the user answers, declines or sends a quick reply, or the call ends elsewhere. Esc declines and Enter answers.
// The rings pulse unless the system asks for reduced motion (call.css). Video answers wait for the camera.
import { forwardRef, useEffect, useRef, useState } from 'react';
import { Lock, Phone, PhoneOff, ShieldCheck } from 'lucide-react';
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogTitle } from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';
import { CHECK_CLASS } from '@/components/settings/settings-ui';
import { UserAvatar } from '@/components/user-avatar';
import { acceptIncoming, declineIncoming, replyAndDecline, useIncoming } from '@/lib/calls';
import { startRingtone, stopRingtone } from '@/lib/sounds';
import type { IncomingCall } from '@/lib/types';
import { cn } from '@/lib/utils';
import './call.css';

const QUICK_REPLIES = ['Joining in 5 minutes', 'Can’t make it, catch me up', 'Text me instead'];

// A list of names as a sentence: "Maya, Omar and Rana".
function joinNames(names: string[]): string {
  return names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : (names[0] ?? '');
}

export function IncomingCallDialog() {
  const call = useIncoming((s) => s.call);
  // The ringtone runs while the dialog is open, for any call that rings this device.
  useEffect(() => {
    if (!call) return;
    startRingtone();
    return () => stopRingtone();
  }, [call]);
  if (!call) return null;
  return <Ringing key={call.call_id} call={call} />;
}

function Ringing({ call }: { call: IncomingCall }) {
  const [muted, setMuted] = useState(false);
  const [settled, setSettled] = useState(false); // the first answer, decline or reply wins
  const answerRef = useRef<HTMLButtonElement>(null);
  const group = call.scope === 'group';
  const caller = call.caller.display_name;
  const title = group ? call.name : caller;
  const names = call.members_in.map((m) => m.display_name);
  const inCall = group && names.length > 0;

  // Closes the dialog for this call once the answer is in. The store clears it when the call stops ringing.
  const finish = (ok: Promise<boolean>) => {
    if (settled) return;
    setSettled(true);
    void ok.then((done) => {
      if (done) useIncoming.setState({ call: null });
      else setSettled(false);
    });
  };
  const decline = () => finish(declineIncoming(call.call_id));
  const answer = () => finish(acceptIncoming(call, muted));
  const reply = (text: string) => finish(replyAndDecline(call, text));

  return (
    <AlertDialog open>
      <AlertDialogContent
        aria-describedby="incoming-sub"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          answerRef.current?.focus();
        }}
        onEscapeKeyDown={(e) => {
          e.preventDefault();
          decline();
        }}
        onKeyDown={(e) => {
          // Enter answers. On a focused button it already activates that button.
          if (e.key === 'Enter' && e.target === e.currentTarget) {
            e.preventDefault();
            answer();
          }
        }}
        className="incoming w-[540px] max-w-[calc(100vw-32px)] rounded-xl border-0 bg-chat p-0 shadow-[0_24px_64px_rgb(0_0_0_/_.45)]"
      >
        <div className="flex flex-col items-center gap-s-2 px-s-8 pt-s-8 pb-s-6 text-center">
          <div className="relative grid size-[200px] place-items-center" aria-hidden="true">
            <span className="ring-pulse absolute inset-0 rounded-full border-2 border-success" />
            <span className="ring-pulse ring-pulse-2 absolute inset-0 rounded-full border-2 border-success" />
            <span className="ring-pulse ring-pulse-3 absolute inset-0 rounded-full border-2 border-success" />
            {group ? (
              <div className="flex">
                {call.members_in.slice(0, 2).map((m, i) => (
                  <UserAvatar key={m.root_id} name={m.display_name} seed={m.root_id} size="xl" className={cn(i > 0 && '-ml-[18px]')} />
                ))}
              </div>
            ) : (
              <UserAvatar name={caller} seed={call.caller.root_id} size="xl" />
            )}
          </div>
          <p className="mt-s-2 text-meta font-semibold tracking-[.04em] text-success-text uppercase">{group ? 'Group call' : 'Incoming call'}</p>
          <AlertDialogTitle className="text-[25px] leading-8 font-bold text-text-strong">{title}</AlertDialogTitle>
          <AlertDialogDescription id="incoming-sub" className="max-w-[360px] text-ui leading-[21px] text-muted-foreground">
            {group
              ? `${caller} started a voice call in your group. Only the group’s ${call.total} members can join.`
              : `${caller} is calling you.`}
          </AlertDialogDescription>

          <div className="flex flex-wrap justify-center gap-s-2">
            <span className="inline-flex h-[26px] items-center gap-1.5 rounded-full bg-success-tint px-2.5 text-meta font-semibold text-success-text">
              <Lock className="size-3.5" aria-hidden="true" />
              End-to-end encrypted
            </span>
            {call.verified ? (
              <span className="inline-flex h-[26px] items-center gap-1.5 rounded-full bg-success-tint px-2.5 text-meta font-semibold text-success-text">
                <ShieldCheck className="size-3.5" aria-hidden="true" />
                Safety number verified
              </span>
            ) : null}
            <span className="inline-flex h-[26px] items-center rounded-full bg-elevated px-2.5 text-meta font-semibold text-muted-foreground">
              {group ? `${call.in_call} of ${call.total} in call` : 'Direct call'}
            </span>
          </div>

          {inCall ? (
            <div className="flex items-center gap-s-3 rounded-lg bg-panel px-3.5 py-2 text-ui text-text">
              <span className="flex">
                {call.members_in.slice(0, 3).map((m, i) => (
                  <UserAvatar key={m.root_id} name={m.display_name} seed={m.root_id} size="sm" className={cn(i > 0 && '-ml-2')} />
                ))}
              </span>
              <span>{`${joinNames(names)} ${names.length > 1 ? 'are' : 'is'} in the call`}</span>
            </div>
          ) : null}

          <div className="mt-s-4 flex gap-s-10">
            <AnswerButton label={group ? 'Ignore' : 'Decline'} tone="decline" onClick={decline} disabled={settled} />
            <AnswerButton
              ref={answerRef}
              label={group ? 'Join call' : 'Answer'}
              tone="accept"
              onClick={answer}
              disabled={settled}
            />
          </div>

          <div className="mt-s-4 flex w-full flex-col items-center gap-s-2 border-t border-border pt-s-4">
            <p className="text-meta font-semibold tracking-[.04em] text-muted-foreground uppercase">Can’t talk? Send a quick reply</p>
            <div className="flex flex-wrap justify-center gap-s-2">
              {QUICK_REPLIES.map((text) => (
                <Button
                  key={text}
                  variant="ghost"
                  disabled={settled}
                  onClick={() => reply(text)}
                  className="h-8 rounded-full border border-border px-s-3 text-ui font-normal text-text hover:bg-hover"
                >
                  {text}
                </Button>
              ))}
            </div>
          </div>

          <label className="mt-s-2 inline-flex cursor-pointer items-center gap-s-2 text-meta text-muted-foreground">
            <input
              type="checkbox"
              checked={muted}
              onChange={(e) => setMuted(e.target.checked)}
              className={CHECK_CLASS}
            />
            <span>Start with my microphone muted</span>
          </label>
        </div>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// The round Answer and Decline buttons, with their caption under them.
const AnswerButton = forwardRef<
  HTMLButtonElement,
  { label: string; tone: 'accept' | 'decline'; onClick: () => void; disabled: boolean }
>(function AnswerButton({ label, tone, onClick, disabled }, ref) {
  const Icon = tone === 'accept' ? Phone : PhoneOff;
  return (
    <div className="flex flex-col items-center gap-s-2 text-ui text-text">
      <button
        ref={ref}
        type="button"
        aria-label={label}
        onClick={onClick}
        disabled={disabled}
        className={cn(
          'grid size-[72px] place-items-center rounded-full border-0 text-white focus-visible:outline-2 focus-visible:outline-offset-3 focus-visible:outline-accent-text disabled:opacity-50',
          tone === 'accept' ? 'bg-success' : 'bg-[#d9302c]',
        )}
      >
        <Icon className="size-7" aria-hidden="true" />
      </button>
      <span>{label}</span>
    </div>
  );
});
