// The row pieces the sidebar's lists share: a row's classes, the unread mark and a section heading.
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';

export const rowClass = (active: boolean, unread: boolean) =>
  cn(
    'relative flex h-9 w-full min-w-0 shrink-0 items-center gap-s-2 rounded-lg px-s-2 text-left text-ui font-medium text-muted-foreground hover:bg-hover hover:text-text disabled:pointer-events-none disabled:opacity-50',
    active && 'bg-selected text-text-strong',
    unread && 'font-semibold text-text-strong',
  );

export function UnreadDot() {
  return <span aria-label="unread" className="absolute top-3.5 -left-2 h-2 w-1 rounded-r bg-text-strong" />;
}

// A section's heading. The channel sections are small caps; the direct messages heading is title case, as the reference shows it.
export function SectionHead({ title, action, spaced = false, plain = false }: { title: string; action?: ReactNode; spaced?: boolean; plain?: boolean }) {
  return (
    <div
      className={cn(
        'flex h-8 shrink-0 items-center justify-between pl-s-2 text-muted-foreground',
        plain ? 'text-[13px] font-normal' : 'text-meta font-semibold tracking-[.02em] uppercase',
        spaced && 'mt-s-2',
      )}
    >
      <span className="truncate">{title}</span>
      {action}
    </div>
  );
}
