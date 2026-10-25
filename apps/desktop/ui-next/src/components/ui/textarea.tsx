import * as React from 'react';
import { cn } from '@/lib/utils';

// The current UI's textarea: the field surface with 10 px vertical padding, and a vertical resize only.
function Textarea({ className, ...props }: React.ComponentProps<'textarea'>) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'w-full min-w-0 resize-y rounded-lg border border-input-border bg-input px-s-3 py-2.5 text-body leading-body text-text-strong placeholder:text-muted-foreground focus-visible:shadow-[0_0_0_2px_var(--accent)] focus-visible:outline-none disabled:opacity-50',
        className,
      )}
      {...props}
    />
  );
}

export { Textarea };
