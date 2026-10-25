import * as React from 'react';
import { Dialog as DialogPrimitive } from 'radix-ui';
import { cn } from '@/lib/utils';

// Dialog, restyled to the current UI's .dialog: chat surface, 8 px radius, 520 px wide (600 with `wide`), a 70% scrim.
// The header and body paddings are the current UI's (20 px sides, the title 19 px bold).
const Dialog = DialogPrimitive.Root;
const DialogTrigger = DialogPrimitive.Trigger;
const DialogClose = DialogPrimitive.Close;

function DialogContent({
  className,
  overlayClassName,
  wide = false,
  children,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & { wide?: boolean; overlayClassName?: string }) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className={cn('fixed inset-0 z-50 bg-black/70', overlayClassName)} />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          'fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-32px)] w-[min(520px,92vw)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-lg border border-border bg-chat text-text shadow-[var(--shadow-3)] outline-none',
          wide && 'w-[min(600px,92vw)]',
          className,
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

function DialogHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="dialog-header" className={cn('flex shrink-0 items-start justify-between gap-s-4 px-s-5 pt-s-5', className)} {...props} />;
}

function DialogTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="dialog-title" className={cn('min-w-0 text-head font-bold break-words text-text-strong', className)} {...props} />;
}

function DialogDescription({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return <DialogPrimitive.Description data-slot="dialog-description" className={cn('mt-s-1 text-sm leading-[19px] text-muted-foreground', className)} {...props} />;
}

function DialogBody({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="dialog-body" className={cn('dissent-scroll flex min-h-0 flex-col gap-s-4 overflow-y-auto px-s-5 pt-s-4 pb-s-5', className)} {...props} />;
}

// The footer sits on the sidebar surface and runs to the dialog's edges, as the current UI's `.dialog .row.end` does.
function DialogFooter({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div data-slot="dialog-footer" className={cn('-mx-s-5 -mb-s-5 flex shrink-0 items-center justify-end gap-s-2 bg-sidebar px-s-5 py-s-4', className)} {...props} />
  );
}

export { Dialog, DialogBody, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger };
