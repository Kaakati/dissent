// The verified mark for a website handle, and the status words the profile shows beside one. The mark appears only
// for a pass this app checked itself within VERIFIED_FOR_MS; a failure, a stale pass or no check yet never shows it.
import { BadgeCheck } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { useShellPrefs } from '@/lib/shell-prefs';
import type { HandleCheck, Id } from '@/lib/types';
import { cn } from '@/lib/utils';
import { dnsCheckHint, useWebsiteHandle, verifyHandle } from '@/lib/website-handles';

// The backend's VERIFIED_FOR_MS: a pass older than this is "not checked yet" here too, even before the next answer.
const VERIFIED_FOR_MS = 24 * 60 * 60_000;

// What checking `handle` reveals, in words, with the user's resolver setting.
export function useDnsCheckHint(handle: string): string {
  const publicFallback = useShellPrefs((s) => s.dnsPublicFallback);
  return dnsCheckHint(handle, publicFallback);
}

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

// The mark after a handle, for the member list and message headers: nothing unless the handle verifies. Drawing it never
// looks the handle up.
export function HandleMark({ root, handle, className }: { root: Id; handle: string | null | undefined; className?: string }) {
  const check = useWebsiteHandle(root, handle);
  return isVerified(check) ? <VerifiedMark className={className} /> : null;
}

// A member list handle with its mark. Pointing at it verifies the handle, and its tooltip says what that asks.
export function MemberHandle({ root, handle }: { root: Id; handle: string }) {
  const hint = useDnsCheckHint(handle);
  return (
    <span
      data-member-handle
      title={handle.includes('.') ? hint : undefined}
      onPointerEnter={() => verifyHandle(root, handle)}
      className="flex max-w-[40%] shrink-0 items-center gap-1 text-meta text-muted-foreground"
    >
      <span className="truncate">{`@${handle}`}</span>
      <HandleMark root={root} handle={handle} />
    </span>
  );
}

// The status after a handle on the profile card and the full profile, which verify it when they open: the mark when it
// verifies, "checking" while the lookup runs, "not checked yet" while no fresh check has passed, and "not verified" with
// the reason when the record is missing or wrong. Each says what the check asks.
export function HandleStatus({ root, handle }: { root: Id; handle: string | null | undefined }) {
  const check = useWebsiteHandle(root, handle, true);
  const hint = useDnsCheckHint(check?.handle ?? '');
  if (!check || check.state === 'not_domain') return null;
  const title = (reason: string | null) => (reason ? `${reason} ${hint}` : hint);
  if (isVerified(check)) {
    return (
      <span className="inline-flex" title={hint}>
        <VerifiedMark />
      </span>
    );
  }
  if (check.state === 'failed') {
    return (
      <span data-website-status="failed" className="shrink-0 text-meta text-muted-foreground" title={title(check.reason)}>
        not verified
      </span>
    );
  }
  return (
    <span
      data-website-status={check.checking ? 'checking' : 'unchecked'}
      className="shrink-0 text-meta text-muted-foreground"
      title={title(check.reason)}
    >
      {check.checking ? 'checking' : 'not checked yet'}
    </span>
  );
}
