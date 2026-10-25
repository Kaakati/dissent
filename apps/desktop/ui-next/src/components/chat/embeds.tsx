// Embeds from the sender's sealed message. The client never fetches a URL: a link preview shows only the fields the sender
// included, and an image only as a local path the backend wrote (see imageSrc). Ported from ui/views/chat.js.
import { Layers } from 'lucide-react';
import type { Embed } from '@/lib/types';
import { cn } from '@/lib/utils';
import { imageSrc } from './attachments';
import { spoilerGuard } from './parts';

const HEX = /^#[0-9a-f]{6}$/i;

// A link preview, as previews.rs seals it: the URL, the site, a title, a description and maybe an image.
function LinkCard({ e }: { e: Embed }) {
  if (typeof e.url !== 'string' || !/^https:\/\//i.test(e.url)) return null;
  const src = e.image ? imageSrc(e.image) : null;
  return (
    <article className="embed">
      <div className="embed-site">{e.site ?? ''}</div>
      <a className="embed-title" href={e.url} target="_blank" rel="noopener noreferrer">
        {e.title || e.url}
      </a>
      {e.description ? <p className="embed-desc">{e.description}</p> : null}
      {src ? <img className="embed-img" src={src} alt="" width={e.image_w ?? undefined} height={e.image_h ?? undefined} /> : null}
    </article>
  );
}

// A generic card (kind 'card'): a small accent tile beside the title, the subtitle chip and the tile's label.
function RichCard({ e }: { e: Embed }) {
  const color = e.tile && HEX.test(e.tile.color ?? '') ? e.tile.color : 'var(--elevated)';
  return (
    <article className="rich-card">
      <div className="rich-main">
        {e.tile ? (
          <span className="rich-tile" style={{ background: color }} aria-hidden="true">
            <Layers className="size-4" />
          </span>
        ) : null}
        <div className="rich-text">
          <div className="rich-title">{e.title ?? ''}</div>
          {e.subtitle ? <span className="rich-chip">{e.subtitle}</span> : null}
          {e.tile?.label ? <span className="rich-label">{e.tile.label}</span> : null}
        </div>
      </div>
    </article>
  );
}

// One embed by its kind. Add a kind here with its own card; a kind this client does not know draws nothing, so a newer
// sender cannot break an older client. Embeds without a kind are link previews.
function EmbedNode({ e }: { e: Embed }) {
  switch (e.kind) {
    case undefined: return <LinkCard e={e} />;
    case 'card': return <RichCard e={e} />;
    default: return null;
  }
}

// Up to three cards under a message's text. In a spoiler channel the cards are blurred until they are clicked.
export function EmbedCards({ embeds, hidden, reveal }: { embeds: Embed[]; hidden: boolean; reveal: () => void }) {
  return (
    <div className={cn('embeds', hidden && 'spoiler-hidden')} {...spoilerGuard(hidden, reveal)}>
      {embeds.slice(0, 3).map((e, i) => <EmbedNode key={i} e={e} />)}
    </div>
  );
}
