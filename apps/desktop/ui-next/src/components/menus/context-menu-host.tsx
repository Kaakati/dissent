// Renders the menu opened with openContextMenu(x, y, items) from lib/menu.ts. Mounted once, in App. The menu is a
// DropdownMenu anchored to a point, so the keys, submenus and focus are the same as every other menu. Right-click
// and Shift+F10 (or the Menu key) both pass a point, so a row's onContextMenu can use this as well.
import { useEffect, useRef } from 'react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { closeContextMenu, type OpenMenu, useContextMenu } from '@/lib/menu';
import { MENU_WIDTH, MenuItems } from './menu-items';

export function ContextMenuHost() {
  const menu = useContextMenu((s) => s.menu);
  // A new menu is a new mount, so its focus and position start fresh.
  return menu ? <PointMenu key={menu.id} menu={menu} /> : null;
}

function PointMenu({ menu }: { menu: OpenMenu }) {
  const { x, y, items, placeAbove, opener } = menu;
  const contentRef = useRef<HTMLDivElement>(null);

  // The first row takes the focus, as the current UI does, so the arrow keys work at once. Radix focuses the content
  // itself when it mounts, so this waits for that.
  useEffect(() => {
    const id = window.setTimeout(() => {
      contentRef.current?.querySelector<HTMLElement>('[role^="menuitem"]:not([data-disabled])')?.focus();
    }, 0);
    return () => window.clearTimeout(id);
  }, []);

  return (
    <DropdownMenu open onOpenChange={(open) => { if (!open) closeContextMenu(); }}>
      <DropdownMenuTrigger asChild>
        <span aria-hidden="true" className="pointer-events-none fixed size-0" style={{ left: x, top: y }} />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        ref={contentRef}
        side={placeAbove ? 'top' : 'bottom'}
        align="start"
        sideOffset={0}
        collisionPadding={8}
        aria-label="Context menu"
        className={MENU_WIDTH}
        // A pick may open a dialog, which takes the focus itself; the opener is only restored when nothing else did.
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          if (opener?.isConnected && !document.querySelector('[role="dialog"], [role="alertdialog"]')) opener.focus();
        }}
      >
        <MenuItems entries={items} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
