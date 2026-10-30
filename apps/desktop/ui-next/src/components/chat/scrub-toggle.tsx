// The per-file "Remove identifying details" switch, beside a staged file in the composer and on each file of the Share
// dialog. It shows once the engine has said what the file is (lib/scrub.ts).
import { Tip } from '@/components/ui/tip';
import type { ScrubChoice } from '@/lib/scrub';
import { cn } from '@/lib/utils';

export const SCRUB_LABEL = 'Remove identifying details (location, device, dates, names)';

export function ScrubToggle({ name, choice, onChange, className }: { name: string; choice: ScrubChoice; onChange: (on: boolean) => void; className?: string }) {
  if (choice.scrub === undefined) return null;
  const hint = choice.scrub
    ? choice.scrubbable === false
      ? 'This file’s details can’t be removed. Sending will ask first.'
      : 'Sent as a copy without them, under a generic name. Your file stays as it is.'
    : 'Sent as it is, with its name and details.';
  return (
    <Tip label={hint}>
      <label className={cn('scrub-toggle', choice.scrub && 'on', className)} data-scrub-toggle={name}>
        <input
          type="checkbox"
          checked={choice.scrub}
          aria-label={`${SCRUB_LABEL}: ${name}`}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span>Remove details</span>
      </label>
    </Tip>
  );
}
