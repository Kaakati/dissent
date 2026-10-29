// The verified mark for a website handle, and the status words the profile shows beside one. The mark appears only
// for a pass this app checked itself within VERIFIED_FOR_MS; a failure, a stale pass or no check yet never shows it.
import { BadgeCheck } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import type { HandleCheck, Id } from '@/lib/types';
import { cn } from '@/lib/utils';
import { useWebsiteHandle } from '@/lib/website-handles';

// The backend's VERIFIED_FOR_MS: a pass older than this is "not checked yet" here too, even before the next answer.
const VERIFIED_FOR_MS = 6 * 60 * 60_000;

export const isVerified = (check: HandleCheck | null): boolean =>
  check?.state === 'verified' && check.checked_ms !== null && Date.now() - check.checked_ms < VERIFIED_FOR_MS;

export function VerifiedMark({ className }: { className?: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          data-website-verified
          role="img"
          aria-label="Website handle checked by your app"
          className={cn('inline-grid shrink-0 place-items-center text-link', className)}
        >
          <BadgeCheck className="size-3.5" aria-hidden="true" />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">Checked by your app</TooltipContent>
    </Tooltip>
  );
}

// The mark after a handle, for the member list and message headers: nothing unless the handle verifies.
export function HandleMark({ root, handle, className }: { root: Id; handle: string | null | undefined; className?: string }) {
  const check = useWebsiteHandle(root, handle);
  return isVerified(check) ? <VerifiedMark className={className} /> : null;
}

// The status after a handle on the profile card and the full profile: the mark when it verifies, "not checked yet"
// while no fresh check has passed, and "not verified" with the reason when the record is missing or wrong.
export function HandleStatus({ root, handle }: { root: Id; handle: string | null | undefined }) {
  const check = useWebsiteHandle(root, handle);
  if (!check) return null;
  if (isVerified(check)) return <VerifiedMark />;
  if (check.state === 'failed') {
    return (
      <span data-website-status="failed" className="shrink-0 text-meta text-muted-foreground" title={check.reason ?? undefined}>
        not verified
      </span>
    );
  }
  if (check.state === 'not_domain') return null;
  return (
    <span data-website-status="unchecked" className="shrink-0 text-meta text-muted-foreground" title={check.reason ?? undefined}>
      not checked yet
    </span>
  );
}
