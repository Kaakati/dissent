// Settings > My identity: the avatar decoration. A grid of None, the built-ins this build ships (there may be none), this
// device's recent uploads and Upload, with a live preview on your own avatar. Pointing at a tile previews it; a pick is
// published at once, as the banner is.
import { useEffect, useRef, useState } from 'react';
import { Ban, Upload } from 'lucide-react';
import { Block, Hint } from '@/components/settings/settings-ui';
import { DecorationOverlay } from '@/components/decoration-overlay';
import { UserAvatar } from '@/components/user-avatar';
import { clearAvatarDecoration, setAvatarDecoration, setAvatarDecorationBuiltin, setAvatarDecorationRecent } from '@/lib/actions';
import { api } from '@/lib/api';
import { BUILTIN_DECORATIONS, DECORATION_TYPES, MAX_DECORATION_FILE, builtinDecoration, decorationOf, type Decoration, type RecentDecoration } from '@/lib/decorations';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import { cn } from '@/lib/utils';

const TILE = 'relative grid aspect-square place-items-center rounded-lg bg-elevated text-muted-foreground hover:text-text-strong';
const ring = (on: boolean) => (on ? 'ring-2 ring-accent ring-offset-2 ring-offset-chat' : 'ring-1 ring-border');

const fileToBase64 = (file: File) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(',')[1] ?? '');
    reader.onerror = () => reject(new Error('Could not read that file'));
    reader.readAsDataURL(file);
  });

export function DecorationSection() {
  const me = useShell((s) => s.me);
  const choice = me?.decoration ?? null;
  // undefined: show what is set. null: pointing at None.
  const [pointing, setPointing] = useState<Decoration | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const current = decorationOf(me?.root_id);
  const [recents, setRecents] = useState<RecentDecoration[]>([]);
  // A new upload changes `me`, and then the list.
  useEffect(() => {
    api.avatarDecorationRecents().then(setRecents).catch(() => setRecents([]));
  }, [choice?.decoration_image]);

  const upload = async (file: File) => {
    if (!DECORATION_TYPES.includes(file.type)) {
      toast('Pick a WebP, GIF or PNG image');
      return;
    }
    if (file.size > MAX_DECORATION_FILE) {
      toast('A decoration must be at most 2 MB');
      return;
    }
    setBusy(true);
    try {
      await setAvatarDecoration(await fileToBase64(file));
    } catch (err) {
      toast(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Block title="Avatar decoration">
      <div className="flex flex-wrap items-start gap-s-6">
        <div data-decoration-preview className="p-s-3">
          <UserAvatar name={me?.display_name ?? '?'} seed={me?.root_id} size="xl" decoration={pointing === undefined ? current : pointing} />
        </div>
        <div className="flex min-w-[280px] flex-1 flex-col gap-s-3">
          <div role="radiogroup" aria-label="Avatar decoration" onPointerLeave={() => setPointing(undefined)} className="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-s-2">
            <button
              type="button"
              role="radio"
              aria-checked={!choice}
              aria-label="No decoration"
              title="None"
              data-decoration-tile="none"
              className={cn(TILE, ring(!choice))}
              onPointerEnter={() => setPointing(null)}
              onClick={() => void clearAvatarDecoration()}
            >
              <Ban className="size-5" aria-hidden="true" />
            </button>
            {BUILTIN_DECORATIONS.map((d) => {
              const on = choice?.builtin === d.id;
              const art = builtinDecoration(d.id);
              return (
                <button
                  key={d.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  aria-label={d.name}
                  title={d.name}
                  data-decoration-tile={d.id}
                  className={cn(TILE, ring(on))}
                  onPointerEnter={() => setPointing(art)}
                  onClick={() => void setAvatarDecorationBuiltin(d.id)}
                >
                  {art ? (
                    <span className="relative size-[70%]">
                      <DecorationOverlay decoration={art} loop={false} />
                    </span>
                  ) : null}
                </button>
              );
            })}
            {recents.map((r, i) => {
              const on = choice?.decoration_image === r.decoration_image;
              const art: Decoration = { url: r.decoration_image, still: r.decoration_poster ?? null };
              return (
                <button
                  key={r.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  aria-label={`Your upload ${i + 1}`}
                  title="Your upload"
                  data-decoration-tile={`recent-${i}`}
                  className={cn(TILE, ring(on))}
                  onPointerEnter={() => setPointing(art)}
                  onClick={() => void setAvatarDecorationRecent(r.id)}
                >
                  <span className="relative size-[70%]">
                    <DecorationOverlay decoration={art} loop={false} />
                  </span>
                </button>
              );
            })}
            <button
              type="button"
              aria-label="Upload a decoration"
              title="Upload"
              data-decoration-tile="upload"
              disabled={busy}
              className={cn(TILE, ring(false))}
              onPointerEnter={() => setPointing(undefined)}
              onClick={() => input.current?.click()}
            >
              <Upload className="size-5" aria-hidden="true" />
            </button>
          </div>
          <input
            ref={input}
            data-decoration-input
            type="file"
            accept={DECORATION_TYPES.join(',')}
            className="hidden"
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = '';
              if (file) void upload(file);
            }}
          />
          <Hint>Use only art you have the right to share.</Hint>
          <Hint>Loops around your picture. An animated or still WebP, GIF or PNG with a clear middle, about square, up to 2 MB, 512 pixels and 300 frames. People you share a server or a contact with fetch it from you.</Hint>
        </div>
      </div>
    </Block>
  );
}
