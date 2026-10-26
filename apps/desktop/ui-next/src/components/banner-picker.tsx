// The banner picker: the ten preset swatches and an Upload Image tile that opens the 16:9 crop, with Remove Banner once
// there is one. The checked swatch gets a ring. Server Settings keeps the pick as a draft until Save; Settings > My
// identity publishes it at once. The caller decides through the three callbacks.
import { Image as ImageIcon } from 'lucide-react';
import { chooseImage, type ApplyImage } from '@/components/settings/avatar-editor';
import { BANNER_PRESETS, bannerUrl, type BannerDraft } from '@/lib/banners';
import { cn } from '@/lib/utils';

export function BannerPicker({
  value,
  onPreset,
  onUpload,
  onRemove,
}: {
  value: BannerDraft;
  onPreset: (id: number) => void;
  onUpload: ApplyImage; // an animated GIF also passes its poster frame
  onRemove: () => void;
}) {
  return (
    <>
      <div role="radiogroup" aria-label="Banner" className="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-s-2">
        {BANNER_PRESETS.map((p, i) => {
          const selected = value?.kind === 'preset' && value.id === i;
          return (
            <button
              key={p.name}
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={`${p.name} banner`}
              title={p.name}
              data-banner-swatch={i}
              onClick={() => onPreset(i)}
              className={cn('aspect-video rounded-lg', selected ? 'ring-2 ring-accent ring-offset-2 ring-offset-chat' : 'ring-1 ring-border')}
              style={{ background: p.background }}
            />
          );
        })}
        <button
          type="button"
          role="radio"
          aria-checked={value?.kind === 'image'}
          aria-label="Upload Image"
          data-banner-upload
          onClick={() => chooseImage({ shape: 'banner', onApply: onUpload })}
          className={cn(
            'grid aspect-video place-items-center overflow-hidden rounded-lg bg-elevated bg-cover bg-center text-muted-foreground hover:text-text-strong',
            value?.kind === 'image' ? 'ring-2 ring-accent ring-offset-2 ring-offset-chat' : 'ring-1 ring-dashed ring-border',
          )}
          data-banner-animated={value?.kind === 'image' && value.poster ? '' : undefined}
          style={value?.kind === 'image' ? { backgroundImage: `url("${bannerUrl(value)}")` } : undefined}
        >
          {value?.kind === 'image' ? null : (
            <span className="flex flex-col items-center gap-0.5 text-[11px] font-medium">
              <ImageIcon className="size-4" aria-hidden="true" />
              Upload Image
            </span>
          )}
        </button>
      </div>
      {value ? (
        <button type="button" className="self-start text-ui font-medium text-danger-text hover:underline" onClick={onRemove}>
          Remove Banner
        </button>
      ) : null}
    </>
  );
}
