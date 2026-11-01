// A decoration drawn around an avatar: a little larger than it, centred, and never in the way of a click. Large pictures
// loop. A list row shows the still frame until the row is hovered, so hundreds of rows do not animate at once. Everything
// holds still under reduced motion and while the window is in the background, as animated banners do.
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { bannersStill, subscribeBannerMotion } from '@/lib/banners';
import { stillOf, type Decoration } from '@/lib/decorations';

// What counts as "the row" for a list avatar: the nearest thing the pointer already treats as one unit.
const ROW = 'button, a, li, [role="button"], [role="option"], [role="listitem"], [role="menuitem"], [data-message-row]';

export function DecorationOverlay({ decoration, loop }: { decoration: Decoration; loop: boolean }) {
  const held = useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const [hovered, setHovered] = useState(false);
  const [drawn, setDrawn] = useState<{ url: string; still: string | null } | null>(null);
  const ref = useRef<HTMLImageElement>(null);
  const moving = !held && (loop || hovered);

  useEffect(() => {
    if (loop) return undefined;
    const row = ref.current?.closest(ROW) ?? ref.current?.parentElement?.parentElement;
    if (!row) return undefined;
    const enter = () => setHovered(true);
    const leave = () => setHovered(false);
    row.addEventListener('pointerenter', enter);
    row.addEventListener('pointerleave', leave);
    return () => {
      row.removeEventListener('pointerenter', enter);
      row.removeEventListener('pointerleave', leave);
    };
  }, [loop]);

  // A built-in's still frame is made here, the first time it is wanted. An upload's comes with it.
  const { url, still: given } = decoration;
  const wantsStill = !moving && !given;
  useEffect(() => {
    if (!wantsStill) return undefined;
    let live = true;
    void stillOf(url).then((still) => {
      if (live) setDrawn({ url, still });
    });
    return () => {
      live = false;
    };
  }, [url, wantsStill]);

  const still = given ?? (drawn?.url === url ? drawn.still : null);
  const src = moving ? url : still;
  return (
    <img
      ref={ref}
      data-decoration={moving ? 'moving' : 'still'}
      // An empty still is kept in the tree, so the row's listeners are in place until it has one.
      src={src ?? undefined}
      alt=""
      aria-hidden="true"
      draggable={false}
      decoding="async"
      onError={(e) => {
        e.currentTarget.style.visibility = 'hidden';
      }}
      onLoad={(e) => {
        e.currentTarget.style.visibility = '';
      }}
      className="pointer-events-none absolute -top-[10%] -left-[10%] size-[120%] max-w-none select-none"
    />
  );
}
