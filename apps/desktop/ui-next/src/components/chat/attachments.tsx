// File attachments: the cards in a message, the pictures once they are on this device, and the chips above the composer.
// Nothing here renders user-supplied markup. Names are text, and the only image source is the backend's local path
// converted by Tauri's asset protocol, never a URL taken from a message. Ported from the old UI's views/attachments.js.
import { useState, type ComponentType } from 'react';
import { Download, ExternalLink, File, FileArchive, FileAudio, FileCode, FileText, FileVideo, FolderOpen, Image as ImageIcon, Lock, Pause, RotateCcw, X } from 'lucide-react';
import type { LucideProps } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { ProgressBar } from '@/components/transfers/progress';
import { MAX_FILE_LABEL, extOf, formatSize, iconFor, isExecutable, isImage, isOverLimit, transferText, type FileIconName } from '@/lib/attachments';
import { downloadAttachment, openAttachment, revealAttachment } from '@/lib/chat-actions';
import { openFileDetail } from '@/lib/right';
import { cancelTransfer, pauseTransfer } from '@/lib/transfers';
import type { Attachment, Id, ThumbView } from '@/lib/types';
import { cn } from '@/lib/utils';
import { IconBtn } from './parts';
import type { StagedFile } from '@/lib/chat';

const GLYPH: Record<FileIconName, ComponentType<LucideProps>> = {
  image: ImageIcon,
  'file-audio': FileAudio,
  'file-video': FileVideo,
  'file-archive': FileArchive,
  'file-code': FileCode,
  'file-text': FileText,
  file: File,
};

export function FileGlyph({ name, mime }: { name: string; mime?: string }) {
  const Icon = GLYPH[iconFor({ name, mime })];
  return <Icon aria-hidden="true" />;
}

// The end-to-end lock, at the foot of an image or in a card's meta line.
function LockGlyph() {
  return (
    <span className="att-lock" title="End-to-end encrypted" aria-label="End-to-end encrypted" role="img">
      <Lock aria-hidden="true" />
    </span>
  );
}

// A downloaded picture's source: only the local path the backend wrote, through Tauri's asset protocol.
export function imageSrc(path: string | null | undefined): string | null {
  if (!path) return null;
  const src = window.__TAURI__?.core.convertFileSrc?.(path);
  return typeof src === 'string' ? src : null;
}

// Transfer state, as the card's meta line says it. A failure's reason goes on its own line, under the meta.
function StateNode({ a, unshown }: { a: Attachment; unshown: boolean }) {
  switch (a.state) {
    case 'available': return <span className="att-state">{isImage(a) ? 'Download to view' : 'Not downloaded'}</span>;
    case 'downloading': return <span className="att-state tnum">{transferText(a)}</span>;
    case 'paused': return <span className="att-state tnum">{`Paused · ${transferText(a)}`}</span>;
    case 'failed': return <span className="att-state danger">Download failed</span>;
    case 'disk_full': return <span className="att-state danger tnum">{`Not enough disk space${typeof a.needed === 'number' && a.needed ? ` (needs ${formatSize(a.needed)})` : ''}`}</span>;
    case 'too_large': return <span className="att-state">{`Too large to download (over ${MAX_FILE_LABEL})`}</span>;
    case 'waiting': return <span className="att-state">Waiting for a source</span>;
    case 'sending': return <span className="att-state tnum">{a.status ?? 'Sending'}</span>;
    case 'downloaded':
      if (unshown) return <span className="att-state">Cannot show this picture here. Open it instead.</span>;
      return a.verified ? <span className="att-state" style={{ color: 'var(--success-text)' }}>Saved to Downloads · matches the fingerprint</span> : null;
    default: return null;
  }
}

// The Download button of a file that is not on this device yet: round and green, like the update button in the header.
function DownloadButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <Button
      variant="ghost"
      size="icon"
      title={label}
      aria-label={label}
      onClick={onClick}
      className="att-download size-8 rounded-full bg-success-tint text-success hover:bg-[rgb(35_165_90_/_0.28)] hover:text-success [&_svg]:size-[18px]"
    >
      <Download />
    </Button>
  );
}

// The buttons on a card: download, pause and cancel while it moves, retry, open and show in folder when it is saved.
function CardActions({ a, communityId, messageId }: { a: Attachment; communityId: Id | null; messageId: Id }) {
  const download = () => void downloadAttachment(communityId, messageId, a);
  switch (a.state) {
    case 'available': return <DownloadButton label={`Download ${a.name}`} onClick={download} />;
    case 'downloading':
    case 'sending':
      return a.transfer_id ? (
        <>
          <IconBtn label={`Pause ${a.name}`} icon={Pause} onClick={() => void pauseTransfer(a.transfer_id as Id)} />
          <IconBtn label={`Cancel ${a.name}`} icon={X} onClick={() => void cancelTransfer(a.transfer_id as Id)} />
        </>
      ) : null;
    case 'paused': return <DownloadButton label={`Resume download of ${a.name}`} onClick={download} />;
    case 'failed':
    case 'disk_full':
      return (
        <Button variant="outline" size="sm" className="att-retry" aria-label={`Retry download of ${a.name}`} onClick={download}>
          <RotateCcw aria-hidden="true" />
          Retry
        </Button>
      );
    case 'downloaded':
      return (
        <>
          <IconBtn label={`Open ${a.name}`} icon={ExternalLink} onClick={() => void openAttachment(a)} />
          <IconBtn label={`Show ${a.name} in folder`} icon={FolderOpen} onClick={() => void revealAttachment(a.local_path)} />
        </>
      );
    default: return null;
  }
}

// One file as a card: its icon, name, size, state and buttons. A picture that is not here yet is a card too, never an
// empty or broken image. `unshown` marks a downloaded picture the webview could not show.
function FileCard({ a, communityId, messageId, unshown = false }: { a: Attachment; communityId: Id | null; messageId: Id; unshown?: boolean }) {
  const name = String(a.name ?? 'file');
  const ext = extOf(name);
  const exec = isExecutable(name);
  const downloading = a.state === 'downloading';
  const moving = a.state === 'downloading' || a.state === 'paused' || a.state === 'sending';
  const progress = Math.max(0, Math.min(1, Number(a.progress) || 0));
  const failed = a.state === 'failed' || a.state === 'disk_full';
  return (
    <div
      className={cn('att-card', failed && 'failed', downloading && 'downloading')}
      data-att={a.id}
      data-state={a.state}
    >
      <span className="att-icon">
        <FileGlyph name={name} mime={a.mime} />
      </span>
      <div className="att-body">
        <div className="att-name" title={name}>
          <span
            className="att-name-text"
            title={downloading ? 'Transfer details' : undefined}
            onClick={downloading ? () => openFileDetail(a.id) : undefined}
          >
            {name}
          </span>
          {ext ? <span className={cn('att-ext', exec && 'danger')} title={exec ? 'This is a program' : `.${ext} file`}>{ext.toUpperCase()}</span> : null}
        </div>
        <div className="att-meta">
          <span className="tnum">{formatSize(a.size)}</span>
          <LockGlyph />
          <StateNode a={a} unshown={unshown} />
        </div>
        {failed && a.error ? <div className="att-error">{a.error}</div> : null}
        {moving ? <ProgressBar value={progress} thin className="att-progress" /> : null}
        {a.state === 'waiting' && a.note ? <div className="piece-note">{a.note}</div> : null}
        {downloading && Array.isArray(a.sources) && a.sources.length > 1 ? <div className="att-sources">{`From ${a.sources.length} sources`}</div> : null}
      </div>
      <div className="att-actions">
        <CardActions a={a} communityId={communityId} messageId={messageId} />
      </div>
    </div>
  );
}

// A picture's preview before the picture is downloaded: the sender's thumbnail, softly blurred and sized like a Discord
// embed (at most 400 by 300, aspect kept), over the picture's card with its Download button. Clicking the preview
// downloads too. Shown only once the thumbnail is on this device, so the box is never empty.
function ThumbPreview({ a, thumb, src, onBroken, communityId, messageId }: { a: Attachment; thumb: ThumbView; src: string; onBroken: () => void; communityId: Id | null; messageId: Id }) {
  const name = String(a.name ?? 'file');
  const width = Math.max(1, Number(thumb.width) || 1);
  const height = Math.max(1, Number(thumb.height) || 1);
  const size = { width: Math.min(400, 300 * (width / height)), aspectRatio: `${width} / ${height}` };
  const downloading = a.state === 'downloading';
  return (
    <figure className="att-preview">
      <button
        type="button"
        className="att-preview-btn"
        style={size}
        aria-label={downloading ? `Downloading ${name}` : `Download ${name}`}
        title={`${name} · ${formatSize(a.size)} · End-to-end encrypted`}
        disabled={downloading || a.state === 'too_large'}
        onClick={() => void downloadAttachment(communityId, messageId, a)}
      >
        <img src={src} alt="" loading="lazy" decoding="async" draggable={false} onError={onBroken} />
      </button>
      <FileCard a={a} communityId={communityId} messageId={messageId} />
    </figure>
  );
}

// One attachment in a message: a picture once it is on this device, its preview before that, otherwise a file card.
// An image the webview cannot load falls back to the card, so a message never shows a broken image.
export function AttachmentView({ a, communityId, messageId }: { a: Attachment; communityId: Id | null; messageId: Id }) {
  const [broken, setBroken] = useState<ReadonlySet<string>>(() => new Set());
  const markBroken = (src: string) => setBroken((s) => new Set(s).add(src));
  const name = String(a.name ?? 'file');
  const src = isImage(a) && a.state === 'downloaded' ? imageSrc(a.local_path) : null;
  if (src && !broken.has(src)) {
    return (
      <figure className="att-image">
        <button
          type="button"
          className="att-image-btn"
          aria-label={`Open ${name}`}
          title={`${name} · ${formatSize(a.size)} · End-to-end encrypted`}
          onClick={() => void openAttachment(a)}
        >
          <img src={src} alt={name} loading="lazy" decoding="async" draggable={false} onError={() => markBroken(src)} />
        </button>
        <LockGlyph />
      </figure>
    );
  }
  const thumbSrc = isImage(a) && a.state !== 'downloaded' && a.thumb?.state === 'downloaded' ? imageSrc(a.thumb.path) : null;
  if (thumbSrc && a.thumb && !broken.has(thumbSrc)) {
    return <ThumbPreview a={a} thumb={a.thumb} src={thumbSrc} onBroken={() => markBroken(thumbSrc)} communityId={communityId} messageId={messageId} />;
  }
  return <FileCard a={a} communityId={communityId} messageId={messageId} unshown={src !== null} />;
}

// A file waiting above the composer: its icon, name, size, and a remove button. Over the limit it turns red.
export function StagedChip({ file, onRemove }: { file: StagedFile; onRemove: () => void }) {
  const over = isOverLimit(file);
  const name = String(file.name);
  return (
    <div className={cn('chip-file', over && 'danger')} role="listitem" title={over ? `${name} is over ${MAX_FILE_LABEL} and cannot be sent` : name}>
      <FileGlyph name={name} mime={file.mime} />
      <span className="chip-name">{name}</span>
      <span className="chip-size tnum">{over ? `over ${MAX_FILE_LABEL}` : formatSize(file.size)}</span>
      <IconBtn label={`Remove ${name}`} icon={X} onClick={onRemove} />
    </div>
  );
}
