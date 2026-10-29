// The disclaimer dialog (lib/disclaimer.ts). At first launch, and after the notice changes, it is a gate: a plain summary,
// the full notice in a scrolling box, and "I have read and accept" before Continue; Quit closes the app instead. Esc and
// a click outside do nothing then. From Settings › About it is the same notice, read-only, with Close.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { CHECK_CLASS } from '@/components/settings/settings-ui';
import { api } from '@/lib/api';
import { DISCLAIMER_BLOCKS, DISCLAIMER_TITLE, DISCLAIMER_UPDATED, acceptDisclaimer, closeDisclaimer, useDisclaimer } from '@/lib/disclaimer';
import { toast } from '@/lib/toasts';

// The notice in plain words. The full text below is what applies.
const SUMMARY = [
  'Dissent is free, open-source software. It is peer-to-peer: people connect directly from their own devices.',
  'Nobody operates or moderates the network. The developers run no central server and are not a service provider.',
  'Messages, calls and files are end-to-end encrypted. The developers cannot see, remove or block anything anyone shares.',
  'You are responsible for what you share and for the communities you run, and for following the law where you are.',
  'The software comes with no warranty of any kind. To the extent the law allows, the developers are not liable for any loss or damage from using it.',
  'You agree to hold the developers harmless from claims that arise from how you use it.',
];

function FullText() {
  return (
    <div
      data-disclaimer-text
      tabIndex={0}
      aria-label="Full text"
      className="dissent-scroll max-h-[34vh] min-h-32 overflow-y-auto rounded-md border border-border bg-sidebar px-s-4 py-s-3 text-sm leading-[19px] text-text focus-visible:outline-2 focus-visible:outline-ring"
    >
      {DISCLAIMER_BLOCKS.map((b, i) =>
        b.kind === 'heading' ? (
          <h3 key={i} className="mt-s-4 mb-s-1 text-ui font-semibold text-text-strong first:mt-0">{b.text}</h3>
        ) : (
          <p key={i} className="mb-s-2">{b.text}</p>
        ),
      )}
    </div>
  );
}

export function DisclaimerHost() {
  const mode = useDisclaimer((s) => s.mode);
  const [checked, setChecked] = useState(false);
  const [busy, setBusy] = useState(false);
  if (!mode) return null;
  const gate = mode === 'gate';
  const accept = async () => {
    setBusy(true);
    try {
      await acceptDisclaimer();
    } catch (err) {
      toast(err);
    } finally {
      setBusy(false);
    }
  };
  const quit = () => {
    api.quitApp().catch(() => toast('Close the window to quit.', 'info'));
  };
  return (
    <Dialog open onOpenChange={(open) => (!open && !gate ? closeDisclaimer() : undefined)}>
      <DialogContent
        wide
        data-disclaimer={mode}
        overlayClassName="z-[95]"
        className="z-[95] w-[min(680px,92vw)]"
        onEscapeKeyDown={(e) => gate && e.preventDefault()}
        onPointerDownOutside={(e) => gate && e.preventDefault()}
        onInteractOutside={(e) => gate && e.preventDefault()}
      >
        <DialogHeader>
          <div className="min-w-0">
            <DialogTitle>{DISCLAIMER_TITLE}</DialogTitle>
            <DialogDescription>{`Last updated: ${DISCLAIMER_UPDATED}`}</DialogDescription>
          </div>
        </DialogHeader>
        <DialogBody>
          {gate ? (
            <>
              <p className="text-body text-text-strong">{'Before you start, please read how Dissent works and what you agree to.'}</p>
              <ul data-disclaimer-summary className="m-0 flex list-disc flex-col gap-s-1 pl-s-5 text-ui text-text">
                {SUMMARY.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </>
          ) : null}
          <FullText />
          {gate ? (
            <label className="flex cursor-pointer items-center gap-s-3 text-body text-text-strong">
              <input type="checkbox" className={CHECK_CLASS} checked={checked} onChange={(e) => setChecked(e.target.checked)} data-disclaimer-accept />
              I have read and accept the Disclaimer and Limitation of Liability
            </label>
          ) : null}
          <DialogFooter>
            {gate ? (
              <>
                <Button variant="ghost" onClick={quit}>Quit</Button>
                <Button onClick={() => void accept()} disabled={!checked || busy}>Continue</Button>
              </>
            ) : (
              <Button onClick={closeDisclaimer}>Close</Button>
            )}
          </DialogFooter>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}
