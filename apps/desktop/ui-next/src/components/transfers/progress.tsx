// The determinate progress bar of the current UI (.progress and .progress-fill): a rail-coloured track with an accent
// fill that scales from the left. The value is a fraction from 0 to 1, and the fill animates only with motion allowed.
import { cn } from '@/lib/utils';

export function ProgressBar({ value, thin = false, className }: { value: number; thin?: boolean; className?: string }) {
  const v = Math.min(1, Math.max(0, Number.isFinite(value) ? value : 0));
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(v * 100)}
      className={cn('h-2 overflow-hidden rounded-[4px] bg-rail', thin && 'h-1.5 rounded-[3px]', className)}
    >
      <div
        className="size-full origin-left bg-accent transition-transform duration-200 ease-out motion-reduce:transition-none"
        style={{ transform: `scaleX(${v})` }}
      />
    </div>
  );
}
