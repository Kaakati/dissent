import { CircleAlert, Info } from 'lucide-react';
import { useToasts } from '@/lib/toasts';
import { cn } from '@/lib/utils';

// Transient messages, bottom right. Each one leaves on its own (see lib/toasts.ts).
export function Toaster() {
  const items = useToasts((s) => s.items);
  return (
    <div aria-live="polite" className="fixed right-s-4 bottom-s-4 z-50 flex max-w-[min(360px,90vw)] flex-col gap-s-2">
      {items.map((t) => {
        const Icon = t.kind === 'info' ? Info : CircleAlert;
        return (
          <div
            key={t.id}
            role="status"
            className={cn(
              'flex items-start gap-s-2 rounded-lg border border-border-menu bg-ink px-s-4 py-s-3 text-sm text-text shadow-(--shadow-2)',
              t.kind === 'error' && 'text-text-strong',
            )}
          >
            <Icon className={cn('mt-0.5 size-4 shrink-0', t.kind === 'error' ? 'text-danger-text' : 'text-link')} aria-hidden="true" />
            <span className="min-w-0 break-words">{t.text}</span>
          </div>
        );
      })}
    </div>
  );
}
