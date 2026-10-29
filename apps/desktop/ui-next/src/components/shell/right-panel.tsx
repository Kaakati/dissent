// The right column. It is hidden (and takes no width in the grid) until a view docks something in it. The current UI
// opens it for the member list, the profile panel and call details. Those views are ported in later batches.
import type { ReactNode } from 'react';

export function RightPanel({ children }: { children?: ReactNode }) {
  return (
    <aside
      aria-label="Details"
      hidden={!children}
      // Over the main column's second track, below its header (shell.css, .app-main). Positioned, so it paints above main.
      className="relative col-start-4 row-start-1 mt-[calc(var(--header-h)+var(--s-2))] mr-s-2 mb-s-2 flex min-h-0 flex-col overflow-hidden rounded-lg border border-border bg-panel"
    >
      {children}
    </aside>
  );
}
