// The transfer details panel (views/filedetail.js): one file's progress, its sources, the verified pieces, the fingerprint
// and the encryption rows. It takes its data as props, so the chat view can open it from any downloading attachment.
// DirectTransferDetail is the panel for a direct device-to-device transfer in a DM (the FilesDirect board).
import { Lock, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { imageOf } from '@/lib/images';
import { initials } from '@/lib/identity';
import { useShell } from '@/lib/store';
import { cancelTransfer, pauseTransfer } from '@/lib/transfers';
import { cn } from '@/lib/utils';
import { formatRate, formatSize } from './format';
import { ProgressBar } from './progress';

export interface FileSource {
  display_name?: string;
  note?: string;
  route?: 'direct' | 'relay' | 'seeder';
  rate_bps?: number;
}

// One attachment as the file details read it. `scope` on the panel says whether it sits in a server or a conversation.
export interface FileDetailInfo {
  name: string;
  size: number; // bytes
  progress: number; // 0 to 1
  transfer_id?: string | null;
  fingerprint?: string;
  shared_in?: string;
  sources?: FileSource[];
  pieces?: { total?: number; verified?: number };
}

export interface DirectTransferInfo {
  peer: string;
  sent: number; // bytes
  total: number; // bytes
  speed_bps: number;
  time_left_s: number;
}

const ROUTE_LABEL: Record<NonNullable<FileSource['route']>, string> = { direct: 'Direct', relay: 'Via relay', seeder: 'Seeder' };

// The header row shared by both panels: a title and a close button.
function Head({ title, onClose }: { title: string; onClose: () => void }) {
  return (
    <div className="flex items-center justify-between">
      <h3 className="text-head font-bold text-text-strong">{title}</h3>
      <Tip label="Close">
        <Button variant="ghost" size="icon" aria-label="Close transfer details" onClick={onClose}>
          <X className="size-4" />
        </Button>
      </Tip>
    </div>
  );
}

const SECTION = 'text-meta font-semibold tracking-[0.02em] text-muted-foreground uppercase';

export function FileDetail({ file, scope, onClose }: { file: FileDetailInfo; scope: 'server' | 'conversation'; onClose: () => void }) {
  const sources = Array.isArray(file.sources) ? file.sources : [];
  const total = Math.max(0, Number(file.pieces?.total) || 0);
  const verified = Math.min(total, Math.max(0, Number(file.pieces?.verified) || 0));
  const inConversation = scope === 'conversation';
  const size = Number(file.size) || 0;
  const progress = Math.min(1, Math.max(0, Number(file.progress) || 0));
  const rate = sources.reduce((sum, src) => sum + (Number(src.rate_bps) || 0), 0);
  const left = rate > 0 ? Math.round((size * (1 - progress)) / rate) : null;
  const meta = `${formatSize(size * progress)} of ${formatSize(size)}`
    + (rate > 0 ? ` · ${formatRate(rate)}` : '')
    + (left !== null ? ` · about ${left} s left` : '');
  const id = file.transfer_id;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-s-3 overflow-y-auto p-s-4" aria-label="Transfer details">
      <Head title="Transfer" onClose={onClose} />
      <div className="text-ui font-semibold break-words text-text-strong [overflow-wrap:anywhere]">{file.name}</div>
      <div className="text-meta tabular-nums text-muted-foreground">{meta}</div>
      <ProgressBar value={progress} className="m-0" />

      <div className="flex items-start gap-s-2 rounded-lg bg-panel p-3 text-[12px] leading-[18px] text-muted-foreground">
        <Lock className="mt-px size-4 flex-none text-success" aria-hidden="true" />
        <span>
          <strong className="font-bold">{inConversation ? 'Sent directly' : 'Shared in a server'}</strong>
          <br />
          <span>{file.shared_in ?? (inConversation ? 'Only this conversation gets the key.' : 'Only its members get the key.')}</span>
        </span>
      </div>

      <h4 className={SECTION}>{sources.length === 1 ? 'Downloading directly' : `Downloading from ${sources.length} sources`}</h4>
      <div className="flex flex-col">
        {sources.map((src, i) => (
          <div key={`${src.display_name ?? ''}-${i}`} className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-s-2 py-2 text-ui shadow-[inset_0_-1px_0_var(--border)]">
            <span className="flex min-w-0 flex-col">
              <span className="truncate whitespace-nowrap text-text">{src.display_name ?? 'Unknown'}</span>
              {src.note ? <span className="text-meta text-muted-foreground">{src.note}</span> : null}
            </span>
            <span className="text-meta text-muted-foreground">{ROUTE_LABEL[src.route ?? 'direct'] ?? 'Direct'}</span>
            <span className="text-meta text-muted-foreground tabular-nums">{src.rate_bps ? formatRate(src.rate_bps) : ''}</span>
          </div>
        ))}
      </div>

      <h4 className={SECTION}>Checked as it arrives</h4>
      {total ? <PieceGrid total={total} verified={verified} /> : null}
      <div className="text-meta tabular-nums text-muted-foreground">
        {`${verified.toLocaleString('en-US')} of ${total.toLocaleString('en-US')} pieces verified against the file’s fingerprint`}
      </div>

      {id ? (
        <div className="flex gap-s-2">
          <Button variant="outline" className="flex-1" onClick={() => void pauseTransfer(id)}>Pause</Button>
          <Button variant="outline" className="flex-1" onClick={() => void cancelTransfer(id)}>Cancel</Button>
        </div>
      ) : null}

      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-s-2 text-[12px] leading-[18px]">
        {file.fingerprint ? (
          <>
            <dt className="text-meta text-muted-foreground">Fingerprint</dt>
            <dd className="font-mono text-meta break-words text-text [overflow-wrap:anywhere]">{file.fingerprint}</dd>
          </>
        ) : null}
        <Row label="Encryption" value={inConversation ? 'Its own key, sent sealed to this conversation only' : 'Its own key, sent sealed to members only'} />
        <Row label={inConversation ? 'Relay' : 'Relay, seeder'} value="See encrypted bytes and size, never the name or content" />
        <Row label="Playback" value="Opens when the whole file has arrived" />
      </dl>
    </div>
  );
}

// One row of the encryption and playback facts.
function Row({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt className="text-meta text-muted-foreground">{label}</dt>
      <dd className="text-meta break-words text-text [overflow-wrap:anywhere]">{value}</dd>
    </>
  );
}

// One cell per piece, up to 96; past that, the cells show the verified share of 96. Verified pieces are success green.
function PieceGrid({ total, verified }: { total: number; verified: number }) {
  const n = Math.min(total, 96);
  const filled = total ? Math.round((verified / total) * n) : 0;
  return (
    <div className="grid grid-cols-[repeat(24,minmax(0,1fr))] gap-0.5">
      {Array.from({ length: n }, (_, i) => (
        <span key={i} className={cn('h-2 rounded-[1px] bg-elevated', i < filled && 'bg-success')} />
      ))}
    </div>
  );
}

// A direct transfer between two of your devices: the peers, the path, the speed, and the stop controls.
export function DirectTransferDetail({
  file,
  direct,
  onClose,
}: {
  file: Pick<FileDetailInfo, 'progress' | 'transfer_id'>;
  direct: DirectTransferInfo;
  onClose: () => void;
}) {
  const me = useShell((s) => s.me);
  const pct = Math.round((Number(file.progress) || 0) * 100);
  const sent = `${(direct.sent / 1024 ** 2).toFixed(1)} / ${formatSize(direct.total)}`;
  const id = file.transfer_id;
  const myImage = me ? imageOf(me.root_id) : null;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-s-3 overflow-y-auto p-s-4" aria-label="Transfer details">
      <Head title="Direct transfer" onClose={onClose} />
      <div className="flex items-center gap-s-2">
        <span className="flex flex-col items-center gap-1 text-meta text-text">
          <Avatar initials={initials(me?.display_name)} image={myImage} />
          <span>You</span>
        </span>
        <span className="h-0.5 flex-1 rounded-[1px] bg-success" aria-hidden="true" />
        <span className="flex flex-col items-center gap-1 text-meta text-text">
          <Avatar initials={direct.peer.slice(0, 2).toUpperCase()} image={null} />
          <span>{direct.peer}</span>
        </span>
      </div>
      <h4 className={SECTION}>{`Straight to ${direct.peer}’s device`}</h4>
      <p className="text-meta text-muted-foreground">Your two devices found each other and connected directly, so nothing passes through anyone else.</p>
      <div className="grid grid-cols-2 gap-s-2">
        <Stat label="Path" value="Direct" />
        <Stat label="Speed" value={`${formatSize(direct.speed_bps)}/s`} />
        <Stat label="Sent" value={sent} />
        <Stat label="Time left" value={`~${direct.time_left_s} s`} />
      </div>
      <ProgressBar value={pct / 100} className="m-0" />
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-s-2 text-[12px] leading-[18px]">
        <Row label="Stored on" value={`Your device and ${direct.peer}’s. No server keeps a copy.`} />
        <Row label="Encryption" value={`The file has its own key. Only ${direct.peer} receives it.`} />
        <Row label="If it stalls" value="After 30 s of silence Dissent reconnects and carries on." />
      </dl>
      {id ? (
        <div className="mt-auto flex gap-s-2">
          <Button variant="outline" className="flex-1" onClick={() => void pauseTransfer(id)}>Pause</Button>
          <Button variant="outline" className="flex-1 text-white" onClick={() => void cancelTransfer(id)}>Cancel</Button>
        </div>
      ) : null}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-0.5 rounded-lg bg-panel px-3 py-2.5">
      <span className="text-meta text-muted-foreground">{label}</span>
      <span className="text-ui font-semibold text-text-strong tabular-nums">{value}</span>
    </div>
  );
}

// The 40 px initials disc of the direct transfer panel, with the picture over it when there is one.
function Avatar({ initials: text, image }: { initials: string; image: string | null }) {
  return (
    <span className="relative grid size-10 place-items-center overflow-hidden rounded-full bg-elevated text-[12px] font-semibold text-text-strong">
      {text}
      {image ? <img src={image} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : null}
    </span>
  );
}
