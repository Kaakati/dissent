// The network and DHT line in the title bar's right corner: a status dot and one line of text, with the full numbers in a
// tooltip. It polls net_status every 15 seconds and hides itself while the backend does not answer. Ported from views/status.js.
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { formatNet, type NetView } from './format';

const POLL_MS = 15_000;

export function NetStatusLine() {
  const [view, setView] = useState<NetView | null>(null);

  useEffect(() => {
    let live = true;
    const refresh = () => {
      api
        .netStatus()
        .then((s) => {
          if (live) setView(formatNet(s));
        })
        .catch(() => {
          if (live) setView(null);
        });
    };
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);

  if (!view) return null;
  // The line is not clickable, so its parts drag the window (Tauri checks the element that was clicked).
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="flex min-w-0 items-center gap-1.5 text-meta whitespace-nowrap text-muted-foreground tabular-nums" data-tauri-drag-region>
          <span className={cn('size-2 flex-none rounded-full', view.ok ? 'bg-success' : 'bg-offline')} aria-hidden="true" data-tauri-drag-region />
          <span className="truncate" data-tauri-drag-region>{view.text}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="text-left">
        {view.lines.map((line) => (
          <span key={line} className="block">{line}</span>
        ))}
      </TooltipContent>
    </Tooltip>
  );
}
