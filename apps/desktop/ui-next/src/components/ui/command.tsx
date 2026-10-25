import * as React from 'react';
import { Command as CommandPrimitive } from 'cmdk';
import { cn } from '@/lib/utils';

// cmdk, restyled to the quick switcher. The switcher does its own ranking, so cmdk's filter is off there. The
// highlighted row is the red accent, as the current UI's switcher and menus are.
function Command({ className, ...props }: React.ComponentProps<typeof CommandPrimitive>) {
  return <CommandPrimitive data-slot="command" className={cn('flex min-h-0 w-full flex-col overflow-hidden text-text', className)} {...props} />;
}

function CommandInput({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Input>) {
  return (
    <CommandPrimitive.Input
      data-slot="command-input"
      className={cn('h-10 w-full min-w-0 flex-1 bg-transparent text-body text-text-strong placeholder:text-muted-foreground focus-visible:outline-none', className)}
      {...props}
    />
  );
}

function CommandList({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.List>) {
  return <CommandPrimitive.List data-slot="command-list" className={cn('dissent-scroll max-h-[min(360px,60vh)] overflow-y-auto', className)} {...props} />;
}

function CommandEmpty({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Empty>) {
  return <CommandPrimitive.Empty data-slot="command-empty" className={cn('px-s-3 py-s-4 text-sm text-muted-foreground', className)} {...props} />;
}

function CommandItem({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Item>) {
  return (
    <CommandPrimitive.Item
      data-slot="command-item"
      className={cn(
        'flex min-h-10 cursor-pointer items-center gap-s-3 rounded-md px-s-2 text-ui text-text select-none data-[selected=true]:bg-accent data-[selected=true]:text-white data-[disabled=true]:opacity-50 [&_svg]:size-5 [&_svg]:shrink-0',
        className,
      )}
      {...props}
    />
  );
}

export { Command, CommandEmpty, CommandInput, CommandItem, CommandList };
