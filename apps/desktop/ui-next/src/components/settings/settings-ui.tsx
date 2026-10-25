// The parts every settings page is built from: a block with its small uppercase title, the hint line under it, and the
// label row for a checkbox or radio. The sizes are the current UI's (.settings-block, .toggle-row, .radio-row).
import type { ComponentProps, ReactNode } from 'react';
import { cn } from '@/lib/utils';

export const BLOCK_TITLE = 'text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase';

export function Block({ title, children, className }: { title?: string; children: ReactNode; className?: string }) {
  return (
    <section className={cn('flex min-w-0 flex-col gap-2.5', className)}>
      {title ? <h2 className={BLOCK_TITLE}>{title}</h2> : null}
      {children}
    </section>
  );
}

// A group of radio buttons under a legend ("Message density" and the like).
export function Choices({ legend, children }: { legend: string; children: ReactNode }) {
  return (
    <fieldset className="flex min-w-0 flex-col gap-2.5 border-0 p-0 m-0">
      <legend className={cn(BLOCK_TITLE, 'p-0')}>{legend}</legend>
      {children}
    </fieldset>
  );
}

export function Hint({ className, ...props }: ComponentProps<'p'>) {
  return <p className={cn('text-sm leading-[19px] text-muted-foreground', className)} {...props} />;
}

// A checkbox or radio with its label, one 32 px row.
export function CheckRow({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <label className={cn('flex h-8 cursor-pointer items-center gap-s-3 text-body text-text-strong', className)}>{children}</label>
  );
}

// The native box takes the field surface and edge, as `input` does in the current UI.
export const CHECK_CLASS = 'm-0 size-5 shrink-0 accent-accent border border-input-border bg-input disabled:opacity-50';

export const ROW = 'flex items-center gap-s-2';
