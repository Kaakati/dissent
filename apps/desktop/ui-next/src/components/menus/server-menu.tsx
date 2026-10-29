// The server's menu, opened from the name at the top of the channel list. It is the same menu as the server icon's on
// the rail (server-rail-menu.tsx, openServerMenu), with the same items for the same people, opened below the name.
// Enter, Space and ArrowDown open it from the keyboard, as they open any menu button.
import type { KeyboardEvent, MouseEvent, ReactNode } from 'react';
import { Slot } from 'radix-ui';
import { openServerMenu } from '@/components/menus/server-rail-menu';
import type { CommunityView } from '@/lib/types';

const OPEN_KEYS = ['Enter', ' ', 'ArrowDown'];

export function ServerMenu({ community, children }: { community: CommunityView; children: ReactNode }) {
  const open = (target: HTMLElement) => {
    const r = target.getBoundingClientRect();
    void openServerMenu(community.id, r.left, r.bottom + 4);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (!OPEN_KEYS.includes(e.key)) return;
    // The key opens the menu itself, so the button's own click (Enter, Space) does not open it a second time.
    e.preventDefault();
    open(e.currentTarget);
  };
  return (
    <Slot.Root onClick={(e: MouseEvent<HTMLElement>) => open(e.currentTarget)} onKeyDown={onKeyDown}>
      {children}
    </Slot.Root>
  );
}
