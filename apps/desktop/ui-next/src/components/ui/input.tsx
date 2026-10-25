import * as React from 'react';
import { cn } from '@/lib/utils';

// The current UI's field: 40 px, the input surface, a 1 px input edge and 8 px radius (see `input` in style.css).
function Input({ className, type, ...props }: React.ComponentProps<'input'>) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        'h-10 w-full min-w-0 rounded-lg border border-input-border bg-input px-s-3 text-ui text-text-strong placeholder:text-muted-foreground focus-visible:shadow-[0_0_0_2px_var(--accent)] focus-visible:outline-none disabled:opacity-50',
        className,
      )}
      {...props}
    />
  );
}

export { Input };
