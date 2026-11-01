import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import { decorationOf, type Decoration } from '@/lib/decorations';
import { imageOf } from '@/lib/images';
import type { Presence as PresenceState } from '@/lib/types';
import { cn } from '@/lib/utils';
import { DecorationOverlay } from './decoration-overlay';
import { PresenceDot } from './presence';

export type AvatarSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl';

const SIZE: Record<AvatarSize, string> = {
  xs: 'size-4 text-[7px]',
  sm: 'size-6 text-[9px]',
  md: 'size-8 text-[11px]',
  lg: 'size-10 text-sm',
  xl: 'size-20 text-[27px]',
};

// A person's picture, or their initials on a colour that follows the id. A presence glyph goes at the corner when given.
export function UserAvatar({
  name,
  seed,
  size = 'md',
  presence,
  presenceClass,
  speaking = false,
  className,
  decoration,
}: {
  name: string;
  seed?: string | null;
  size?: AvatarSize;
  presence?: PresenceState;
  // Where the presence dot sits, for a picture with a ring of its own. The default is the corner inside the picture.
  presenceClass?: string;
  speaking?: boolean;
  className?: string;
  // The decoration to preview instead of the person's own. Null draws none.
  decoration?: Decoration | null;
}) {
  const id = seed || name;
  const src = imageOf(seed);
  // At 16 px the art would only be noise. The largest pictures loop; a list row moves when it is hovered.
  const art = size === 'xs' ? null : decoration === undefined ? decorationOf(seed) : decoration;
  return (
    <Avatar
      className={cn(
        'overflow-visible font-semibold leading-none text-white',
        SIZE[size],
        speaking && 'ring-2 ring-success',
        className,
      )}
    >
      {src ? <AvatarImage src={src} alt="" draggable={false} className="rounded-full" /> : null}
      <AvatarFallback className="bg-transparent text-white" style={{ background: hueBackground(hueOf(id)), fontSize: 'inherit' }}>
        {initials(name)}
      </AvatarFallback>
      {art ? <DecorationOverlay decoration={art} loop={size === 'xl'} /> : null}
      {presence ? <PresenceDot status={presence} className={presenceClass ?? (size === 'xl' ? 'right-0 bottom-0 size-[18px]' : undefined)} /> : null}
    </Avatar>
  );
}
