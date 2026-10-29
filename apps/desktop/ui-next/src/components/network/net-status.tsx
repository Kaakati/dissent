// The network and DHT line in the title bar's right corner: a status dot and one line of text, with the full numbers in a
// tooltip. The DHT and relay latencies come live from the net_latency event. The rest (node counts, peers, dials) is read
// with net_status every 15 seconds, and the line hides itself while the backend does not answer.
//
// The line takes the room between the centred title and the window controls, and never ellipsizes. When the room is
// short it gives up whole pieces in the order of FIT_STEPS (format.ts): the node count first, the relay host last. A
// hidden copy of every piece in every form is measured, so the choice is made before anything is cut.
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import type { NetStatus } from '@/lib/types';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { FIT_STEPS, formatNet, type NetSegment } from './format';

const POLL_MS = 15_000;
const SEPARATOR = ' · ';
// The status dot (8 px) and the gap after it (6 px).
const DOT_PX = 14;

// The segments shown after the first `steps` of FIT_STEPS: shortened or left out.
export function fitted(segments: NetSegment[], steps: number): NetSegment[] {
  const applied = FIT_STEPS.slice(0, steps);
  return segments.flatMap((seg) => {
    const step = applied.find((s) => s.key === seg.key);
    if (!step) return [seg];
    if (step.to === 'short' && seg.short) return [{ ...seg, text: seg.short }];
    return step.to === 'short' ? [seg] : [];
  });
}

function Segments({ segments }: { segments: NetSegment[] }) {
  return segments.map((seg, i) => (
    <span key={seg.key}>
      {i > 0 ? SEPARATOR : null}
      <span className={cn(seg.tone === 'warn' && 'text-warn', seg.tone === 'slow' && 'text-danger-text')} data-net-tone={seg.tone ?? 'normal'} data-net-key={seg.key}>
        {seg.text}
      </span>
    </span>
  ));
}

// The fewest FIT_STEPS that make the line fit `room`, from the measured width of each piece. All of them when even the
// last form does not fit; the box then clips rather than overlap the controls.
function stepsToFit(segments: NetSegment[], room: number, width: (text: string) => number): number {
  const sep = width(SEPARATOR);
  for (let steps = 0; steps <= FIT_STEPS.length; steps++) {
    const shown = fitted(segments, steps);
    const total = DOT_PX + shown.reduce((sum, seg, i) => sum + width(seg.text) + (i > 0 ? sep : 0), 0);
    if (total <= room) return steps;
  }
  return FIT_STEPS.length;
}

export function NetStatusLine() {
  const [status, setStatus] = useState<NetStatus | null>(null);
  const latest = useShell((s) => s.netLatency);
  const box = useRef<HTMLSpanElement>(null);
  const measure = useRef<HTMLSpanElement>(null);
  const [steps, setSteps] = useState(0);

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

  const view = status ? formatNet(status, latest) : null;
  const shape = view ? JSON.stringify(view.segments) : '';

  // Re-fit when the pieces change and whenever the room changes (a resize, the window controls, the title).
  useLayoutEffect(() => {
    const room = box.current;
    const probe = measure.current;
    if (!room || !probe || !view) return;
    const fit = () => {
      const widths = new Map<string, number>();
      for (const node of probe.querySelectorAll<HTMLElement>('[data-measure]')) widths.set(node.dataset.measure ?? '', node.getBoundingClientRect().width);
      setSteps(stepsToFit(view.segments, room.clientWidth, (text) => widths.get(text) ?? 0));
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(room);
    return () => observer.disconnect();
    // `shape` stands for view.segments, which is a new array on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape]);

  if (!view) return null;
  const shown = fitted(view.segments, steps);
  const forms = [...new Set([SEPARATOR, ...view.segments.flatMap((seg) => (seg.short ? [seg.text, seg.short] : [seg.text]))])];
  // The line is not clickable, so its parts drag the window (Tauri checks the element that was clicked).
  return (
    <span ref={box} className="relative flex min-w-0 flex-1 justify-end overflow-hidden" data-tauri-drag-region data-net-room>
      {/* Fixed, so the copy never widens the box it measures for (an absolute one would count as overflow). */}
      <span ref={measure} aria-hidden="true" className="pointer-events-none invisible fixed top-0 left-0 flex text-meta whitespace-pre tabular-nums">
        {forms.map((text) => (
          <span key={text} data-measure={text}>{text}</span>
        ))}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="flex shrink-0 items-center gap-1.5 text-meta whitespace-nowrap text-muted-foreground tabular-nums" data-tauri-drag-region data-net-line data-net-steps={steps}>
            <span className={cn('size-2 flex-none rounded-full', view.ok ? 'bg-success' : 'bg-offline')} aria-hidden="true" data-tauri-drag-region />
            <span data-tauri-drag-region data-net-text>
              <Segments segments={shown} />
            </span>
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="text-left">
          {view.lines.map((line) => (
            <span key={line} className="block">{line}</span>
          ))}
        </TooltipContent>
      </Tooltip>
    </span>
  );
}
