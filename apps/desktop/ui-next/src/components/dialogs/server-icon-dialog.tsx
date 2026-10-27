// The server icon dialog, from the server menu (owners and admins). The icon is a rounded picture on the server rail, in the
// switcher and in invites. Change icon picks and crops a picture; Remove icon clears it. Ported from openServerIconDialog
// in the old UI's views/avatar.js.
import { useSyncExternalStore } from 'react';
import { Image as ImageIcon, Trash2 } from 'lucide-react';
import { bannersStill, subscribeBannerMotion } from '@/lib/banners';
import { chooseImage } from '@/components/settings/avatar-editor';
import { Button } from '@/components/ui/button';
import { clearCommunityIcon, setCommunityIcon, setCommunityIconAnimated } from '@/lib/actions';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import { iconSrc } from '@/lib/images';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import type { CommunitySummary, Id } from '@/lib/types';

// The icon with Change icon and Remove icon. Also the icon block of Server Settings.
export function ServerIconEditor({ communityId, name }: { communityId: Id; name: string }) {
  // Subscribing to the list makes the picture redraw once the icon has been saved and the list reloaded.
  useShell((s) => s.communities);
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const src = iconSrc(communityId);
  return (
    <div className="flex items-center gap-s-4">
      <span
        className="relative grid size-20 shrink-0 place-items-center overflow-hidden rounded-[26px] text-[27px] font-bold text-white"
        style={{ background: hueBackground(hueOf(communityId)) }}
      >
        {src ? <img src={src} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : initials(name)}
      </span>
      <div className="flex flex-col gap-s-2">
        <Button
          variant="outline"
          onClick={() =>
            chooseImage({
              shape: 'rounded',
              animated: true,
              onApply: (bytes, animated) =>
                animated ? setCommunityIconAnimated(communityId, bytes, animated.poster) : setCommunityIcon(communityId, bytes),
            })
          }
        >
          <ImageIcon />
          Change icon
        </Button>
        {src ? (
          <Button variant="secondary" className="text-danger-text" onClick={() => void clearCommunityIcon(communityId)}>
            <Trash2 />
            Remove icon
          </Button>
        ) : null}
      </div>
    </div>
  );
}

export function openServerIconDialog(community: CommunitySummary): void {
  openDialog({
    title: 'Server icon',
    sub: community.name,
    content: () => <ServerIconEditor communityId={community.id} name={community.name} />,
  });
}
