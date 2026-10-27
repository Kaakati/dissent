// The network and DHT line in the title bar's right corner: a status dot and one line of text, with the full numbers in a
// tooltip. The DHT and relay latencies come live from the net_latency event. The rest (node counts, peers, dials) is read
// with net_status every 15 seconds, and the line hides itself while the backend does not answer.
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import type { NetStatus } from '@/lib/types';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { formatNet, type NetSegment } from './format';

const POLL_MS = 15_000;

function Segments({ segments }: { segments: NetSegment[] }) {
  return segments.map((seg, i) => (
    <span key={i}>
      {i > 0 ? ' · ' : null}
      <span className={cn(seg.tone === 'warn' && 'text-warn', seg.tone === 'slow' && 'text-danger-text')} data-net-tone={seg.tone ?? 'normal'}>
        {seg.text}
      </span>
    </span>
  ));
}

export function NetStatusLine() {
  const [status, setStatus] = useState<NetStatus | null>(null);
  const latest = useShell((s) => s.netLatency);

  useEffect(() => {
    let live = true;
    const refresh = () => {
      api
        .netStatus()
        .then((s) => {
          if (live) setStatus(s);
        })
        .catch(() => {
          if (live) setStatus(null);
        });
    };
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);

  if (!status) return null;
  const view = formatNet(status, latest);
  // The line is not clickable, so its parts drag the window (Tauri checks the element that was clicked).
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="flex min-w-0 items-center gap-1.5 text-meta whitespace-nowrap text-muted-foreground tabular-nums" data-tauri-drag-region data-net-line>
          <span className={cn('size-2 flex-none rounded-full', view.ok ? 'bg-success' : 'bg-offline')} aria-hidden="true" data-tauri-drag-region />
          <span className="truncate" data-tauri-drag-region>
            <Segments segments={view.segments} />
          </span>
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
