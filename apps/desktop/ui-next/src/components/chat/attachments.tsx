// File attachments: the cards in a message, the image thumbnails of downloaded pictures, and the chips above the composer.
// Nothing here renders user-supplied markup. Names are text, and the only image source is the backend's local path
// converted by Tauri's asset protocol, never a URL taken from a message. Ported from ui/views/attachments.js.
import type { ComponentType } from 'react';
import { Download, ExternalLink, File, FileArchive, FileAudio, FileCode, FileText, FileVideo, FolderOpen, Image as ImageIcon, Lock, Pause, RotateCcw, X } from 'lucide-react';
import type { LucideProps } from 'lucide-react';
import { ProgressBar } from '@/components/transfers/progress';
import { MAX_FILE_LABEL, extOf, formatSize, iconFor, isExecutable, isImage, isOverLimit, transferText, type FileIconName } from '@/lib/attachments';
import { downloadAttachment, openAttachment, revealAttachment } from '@/lib/chat-actions';
import { openFileDetail } from '@/lib/right';
import { cancelTransfer, pauseTransfer } from '@/lib/transfers';
import type { Attachment, Id } from '@/lib/types';
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

// Transfer state, as the card's meta line says it.
function StateNode({ a }: { a: Attachment }) {
  switch (a.state) {
    case 'downloading': return <span className="att-state tnum">{transferText(a)}</span>;
    case 'paused': return <span className="att-state tnum">{`Paused · ${transferText(a)}`}</span>;
    case 'failed': return <span className="att-state danger">Download failed</span>;
    case 'disk_full': return <span className="att-state danger tnum">{`Not enough disk space${a.needed ? ` (needs ${formatSize(a.needed)})` : ''}`}</span>;
    case 'too_large': return <span className="att-state">{`Too large to download (over ${MAX_FILE_LABEL})`}</span>;
    case 'waiting': return <span className="att-state">Waiting for a source</span>;
    case 'sending': return <span className="att-state tnum">{a.status ?? 'Sending'}</span>;
    case 'downloaded': return a.verified ? <span className="att-state" style={{ color: 'var(--success-text)' }}>Saved to Downloads · matches the fingerprint</span> : null;
    default: return null;
  }
}

// The buttons on a card: download, pause and cancel while it moves, retry, open and show in folder when it is saved.
function CardActions({ a, communityId, messageId }: { a: Attachment; communityId: Id | null; messageId: Id }) {
  const download = () => void downloadAttachment(communityId, messageId, a);
  switch (a.state) {
    case 'available': return <IconBtn label={`Download ${a.name}`} icon={Download} onClick={download} />;
    case 'downloading':
    case 'sending':
      return a.transfer_id ? (
        <>
          <IconBtn label={`Pause ${a.name}`} icon={Pause} onClick={() => void pauseTransfer(a.transfer_id as Id)} />
          <IconBtn label={`Cancel ${a.name}`} icon={X} onClick={() => void cancelTransfer(a.transfer_id as Id)} />
        </>
      ) : null;
    case 'paused': return <IconBtn label={`Resume download of ${a.name}`} icon={Download} onClick={download} />;
    case 'failed':
    case 'disk_full': return <IconBtn label={`Retry download of ${a.name}`} icon={RotateCcw} onClick={download} />;
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

// One attachment in a message: a picture once it is downloaded, otherwise a file card.
export function AttachmentView({ a, communityId, messageId }: { a: Attachment; communityId: Id | null; messageId: Id }) {
  const name = String(a.name ?? 'file');
  const src = isImage(a) && a.state === 'downloaded' ? imageSrc(a.local_path) : null;
  if (src) {
    return (
      <figure className="att-image">
        <button
          type="button"
          className="att-image-btn"
          aria-label={`Open ${name}`}
          title={`${name} · ${formatSize(a.size)} · End-to-end encrypted`}
          onClick={() => void openAttachment(a)}
        >
          <img src={src} alt={name} loading="lazy" decoding="async" draggable={false} />
        </button>
        <LockGlyph />
      </figure>
    );
  }
  const ext = extOf(name);
  const exec = isExecutable(name);
  const downloading = a.state === 'downloading';
  const moving = a.state === 'downloading' || a.state === 'paused' || a.state === 'sending';
  const progress = Math.max(0, Math.min(1, Number(a.progress) || 0));
  return (
    <div
      className={cn('att-card', a.state === 'failed' && 'failed', downloading && 'downloading')}
      data-att={a.id}
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
          <StateNode a={a} />
        </div>
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
