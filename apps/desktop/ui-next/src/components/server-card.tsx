// The server card: the banner (a preset or a picture), the rounded icon over its bottom edge, the name, the member and
// online counts, the month it was founded, the description and the traits. Server Settings shows it as the live preview,
// and an invite card is the same card with a Join button in `footer`.
import { useSyncExternalStore, type ReactNode } from 'react';
import { Radio } from 'lucide-react';
import { bannerBackground, bannersStill, subscribeBannerMotion, type BannerDraft } from '@/lib/banners';
import { boostLabel } from '@/lib/boosts';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import type { Trait } from '@/lib/types';
import { cn } from '@/lib/utils';

const MONTH = new Intl.DateTimeFormat('en', { month: 'short', year: 'numeric' });

export interface ServerCardProps {
  id: string;
  name: string;
  icon: string | null; // a URL the webview can load
  iconPoster?: string | null; // an animated icon's still frame, shown while motion is held
  banner: BannerDraft;
  memberCount: number;
  onlineCount: number;
  createdMs?: number;
  description?: string;
  traits?: Trait[];
  footer?: ReactNode;
  className?: string;
  // 'invite' is the in-chat card: a short banner strip, as in a chat embed. 'preview' is Server Settings' taller one.
  variant?: 'preview' | 'invite';
  boost?: { level: number; hosts: number }; // shown when the server has a level
}

export function ServerCard({ id, name, icon, iconPoster, banner, memberCount, onlineCount, createdMs, description, traits, footer, className, variant = 'preview', boost }: ServerCardProps) {
  useSyncExternalStore(subscribeBannerMotion, bannersStill);
  const background = bannerBackground(banner) ?? hueBackground(hueOf(id));
  // An animated icon holds its still frame while motion is held; otherwise the icon, or the frame when there is no GIF.
  const picture = iconPoster && bannersStill() ? iconPoster : (icon ?? iconPoster);
  return (
    <article data-server-card className={cn('grid w-full overflow-hidden rounded-xl bg-card shadow-[inset_0_0_0_1px_var(--border)]', className)}>
      <div data-card-banner={banner ? banner.kind : 'none'} className={cn('w-full', variant === 'invite' ? 'h-16' : 'h-28')} style={{ background }} />
      <div className="relative grid min-w-0 gap-1 px-4 pb-4 pt-12">
        <span className="absolute -top-9 left-4 grid size-[72px] place-items-center overflow-hidden rounded-[20px] text-2xl font-bold text-white shadow-[0_0_0_4px_var(--card)]" style={{ background: hueBackground(hueOf(id)) }}>
          {picture ? <img src={picture} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : initials(name)}
        </span>
        <div data-card-name className="break-words text-base font-bold text-text-strong">{name || 'Server name'}</div>
        <p className="m-0 flex flex-wrap items-center gap-x-3 text-sm text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block size-2 rounded-full bg-[var(--success)]" aria-hidden="true" />
            {`${onlineCount} Online`}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block size-2 rounded-full bg-[var(--offline)]" aria-hidden="true" />
            {`${memberCount} ${memberCount === 1 ? 'Member' : 'Members'}`}
          </span>
        </p>
        {createdMs ? <p className="m-0 text-xs text-muted-foreground">{`Est. ${MONTH.format(new Date(createdMs))}`}</p> : null}
        {boost?.level ? (
          <p data-card-boost className="m-0 inline-flex items-center gap-1 text-xs font-medium text-text">
            <Radio className="size-3" aria-hidden="true" />
            {boostLabel(boost.level, boost.hosts)}
          </p>
        ) : null}
        {description ? <p data-card-description className="m-0 mt-1 whitespace-pre-wrap break-words text-sm text-text">{description}</p> : null}
        {traits && traits.length ? (
          <ul data-card-traits className="m-0 mt-1 flex list-none flex-wrap gap-1.5 p-0">
            {traits.map((t, i) => (
              <li key={i} className="inline-flex items-center gap-1 rounded-full bg-elevated px-2.5 py-1 text-xs font-medium text-text-strong">
                {t.emoji ? <span aria-hidden="true">{t.emoji}</span> : null}
                {t.label}
              </li>
            ))}
          </ul>
        ) : null}
        {footer}
      </div>
    </article>
  );
}
