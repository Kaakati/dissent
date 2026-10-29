// The app's one hover tooltip, for icon-only buttons: a dark popover with the label in bold, an optional shortcut hint
// and a small arrow, as in the reference client. TipProvider gives every tooltip a 300 ms delay that is skipped while the
// pointer moves between neighbouring buttons, and hides all of them while a drag (rail or channel reorder) is under way.
// Radix opens a tooltip on mouse hover and keyboard focus only, so a touch never shows one.
import { useEffect, type ComponentProps, type ReactElement, type ReactNode } from 'react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';

const DELAY_MS = 300;
// A press that travels this far before it is released is a drag, not a click (the rail and channel lists use 4 to 5 px).
const DRAG_PX = 4;

const MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
// The modifier key as a hint shows it: Ctrl, or ⌘ on a Mac.
export const MOD = MAC ? '⌘' : 'Ctrl';

// Marks the document while a pointer drag is under way (html[data-dragging]); index.css hides every tooltip then.
function useDragGuard(): void {
  useEffect(() => {
    const root = document.documentElement;
    let start: { x: number; y: number } | null = null;
    const up = () => {
      start = null;
      delete root.dataset.dragging;
    };
    const down = (e: PointerEvent) => {
      if (e.button === 0) start = { x: e.clientX, y: e.clientY };
    };
    const move = (e: PointerEvent) => {
      // A release that never reached the window (a capture lost to a removed element) ends the press here instead.
      if ((e.buttons & 1) === 0) {
        if (start || root.dataset.dragging !== undefined) up();
        return;
      }
      if (!start || root.dataset.dragging !== undefined) return;
      if (Math.abs(e.clientX - start.x) > DRAG_PX || Math.abs(e.clientY - start.y) > DRAG_PX) root.dataset.dragging = '';
    };
    window.addEventListener('pointerdown', down, true);
    window.addEventListener('pointermove', move, true);
    window.addEventListener('pointerup', up, true);
    window.addEventListener('pointercancel', up, true);
    return () => {
      window.removeEventListener('pointerdown', down, true);
      window.removeEventListener('pointermove', move, true);
      window.removeEventListener('pointerup', up, true);
      window.removeEventListener('pointercancel', up, true);
      delete root.dataset.dragging;
    };
  }, []);
}

export function TipProvider({ children }: { children: ReactNode }) {
  useDragGuard();
  return (
    // Not hoverable: a tooltip closes as soon as the pointer leaves its button, as in the reference client.
    <TooltipProvider delayDuration={DELAY_MS} skipDelayDuration={DELAY_MS} disableHoverableContent>
      {children}
    </TooltipProvider>
  );
}

// Wraps one button. `shortcut` is shown after the label, dimmed. A disabled button gets no pointer events, so `wrap`
// puts it in a span that carries the hover instead (the button keeps its own focus and label). Any other props (and the
// ref) pass through to the button, so a menu trigger with asChild can sit outside a Tip: <MenuTrigger asChild><Tip>.
export function Tip({
  label,
  shortcut,
  side = 'top',
  sideOffset = 6,
  wrap = false,
  children,
  ...trigger
}: {
  label: ReactNode;
  shortcut?: string;
  side?: 'top' | 'right' | 'bottom' | 'left';
  sideOffset?: number;
  wrap?: boolean;
  children: ReactElement;
} & Omit<ComponentProps<typeof TooltipTrigger>, 'asChild' | 'children'>) {
  return (
    <Tooltip>
      <TooltipTrigger asChild {...trigger}>{wrap ? <span className="inline-flex [&>:disabled]:pointer-events-none">{children}</span> : children}</TooltipTrigger>
      <TooltipContent side={side} sideOffset={sideOffset} arrow data-tip>
        {label}
        {shortcut ? <kbd className="ml-s-2 font-sans text-meta font-medium text-muted-foreground" data-tip-shortcut>{shortcut}</kbd> : null}
      </TooltipContent>
    </Tooltip>
  );
}
