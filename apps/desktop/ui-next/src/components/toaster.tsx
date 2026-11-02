import { CircleAlert, Info } from 'lucide-react';
import { FileAddsTray } from '@/components/files/file-adds';
import { useToasts } from '@/lib/toasts';
import { cn } from '@/lib/utils';

// Transient messages, bottom right. Each one leaves on its own (see lib/toasts.ts).
export function Toaster() {
  const items = useToasts((s) => s.items);
  return (
    <div aria-live="polite" className="fixed right-s-4 bottom-s-4 z-[85] flex max-w-[min(360px,90vw)] flex-col gap-s-2">
      <FileAddsTray />
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
            {t.actions?.length ? (
              <span className="ml-auto flex shrink-0 flex-col items-end gap-1">
                {t.actions.map((a) => (
                  <button key={a.label} type="button" className="font-semibold text-accent-text hover:underline" onClick={a.run}>
                    {a.label}
                  </button>
                ))}
              </span>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
