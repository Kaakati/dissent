// In-app updates: the update button in the main header when a newer release exists, and the dialog that installs it.
// A portable build cannot update in place, so its dialog sends the user to the release page. Ported from
// views/updater.js.
import { useState, type ReactNode } from 'react';
import { Download } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { api, listenTo } from '@/lib/api';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { UpdateInfo } from '@/lib/types';

const PROGRESS = 'dissent://update-progress';
const mb = (bytes: number) => `${(bytes / 1048576).toFixed(1)} MB`;

// Release notes are plain text. Lines starting with "- " or "* " become a bullet list. Nothing is parsed as HTML.
function ReleaseNotes({ text }: { text?: string }) {
  if (!text || !text.trim()) return <p className="text-muted-foreground">No release notes.</p>;
  const blocks: ReactNode[] = [];
  let list: string[] | null = null;
  const flush = () => {
    if (list) blocks.push(<ul key={`list-${blocks.length}`} className="m-0 list-disc pl-5 [&>li+li]:mt-0.5">{list.map((item, i) => <li key={i}>{item}</li>)}</ul>);
    list = null;
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const bullet = /^[-*•]\s+/.exec(line);
    if (bullet) {
      list = [...(list ?? []), line.slice(bullet[0].length)];
      continue;
    }
    flush();
    if (line) blocks.push(<p key={`line-${blocks.length}`} className="whitespace-pre-wrap">{line}</p>);
  }
  flush();
  return <div className="flex flex-col gap-s-2">{blocks}</div>;
}

// The button in the main header. It stays until the user chooses Later for this release.
export function UpdateButton() {
  const update = useShell((s) => s.update);
  const dismissed = useShell((s) => s.updateDismissed);
  if (!update || dismissed) return null;
  const label = `Update available: v${update.version}`;
  return (
    <Tip label={label} side="bottom">
      <Button
        variant="ghost"
        size="icon"
        aria-label={label}
        onClick={() => openUpdateDialog(update)}
        className="size-8 rounded-full bg-success-tint text-success hover:bg-[rgb(35_165_90_/_0.28)] hover:text-success [&_svg]:size-[18px]"
      >
        <Download />
      </Button>
    </Tip>
  );
}

// Opens the update dialog for a release. Later hides the button for that release.
export function openUpdateDialog(info: UpdateInfo | null): void {
  if (!info) return;
  openDialog({
    title: 'Update available',
    content: (close) => (info.portable ? <PortableBody info={info} close={close} /> : <UpdateBody info={info} close={close} />),
  });
}

// Hides the button for this release. Closing the dialog is the same choice.
function Later({ close, disabled = false }: { close: () => void; disabled?: boolean }) {
  return (
    <Button
      variant="outline"
      disabled={disabled}
      onClick={() => {
        useShell.getState().dismissUpdate();
        close();
      }}
    >
      Later
    </Button>
  );
}

function PortableBody({ info, close }: { info: UpdateInfo; close: () => void }) {
  return (
    <div className="flex flex-col gap-s-4">
      <p>A new version is available. Download the portable build.</p>
      <p className="text-muted-foreground">Version {info.version} is available. You have {info.current_version}.</p>
      <ReleaseNotes text={info.notes} />
      <div className="-mx-s-5 -mb-s-5 flex justify-end gap-s-2 bg-sidebar px-s-5 py-s-4">
        <Later close={close} />
        <Button onClick={() => api.openReleasePage().catch(toast)}>
          <Download />
          Download the portable build
        </Button>
      </div>
    </div>
  );
}

function UpdateBody({ info, close }: { info: UpdateInfo; close: () => void }) {
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [status, setStatus] = useState('');
  const install = async () => {
    setInstalling(true);
    setProgress(0);
    setStatus('Downloading…');
    let unlisten: (() => void) | null = null;
    try {
      unlisten = await listenTo<{ downloaded: number; total?: number | null }>(PROGRESS, (p) => {
        setProgress(p.total ? Math.min(1, p.downloaded / p.total) : 0);
        setStatus(p.total ? `${mb(p.downloaded)} of ${mb(p.total)}` : `${mb(p.downloaded)} downloaded`);
      });
      await api.installUpdate();
      setStatus('Restarting…');
    } catch (err) {
      toast(err);
      setInstalling(false);
      setProgress(null);
      setStatus('');
    } finally {
      unlisten?.();
    }
  };
  return (
    <div className="flex flex-col gap-s-4">
      <p>Version {info.version} is available. You have {info.current_version}.</p>
      {info.date ? <p className="text-muted-foreground">Released {info.date}</p> : null}
      <ReleaseNotes text={info.notes} />
      {progress !== null ? (
        <div className="h-2 overflow-hidden rounded bg-rail">
          <div
            className="size-full origin-left bg-accent transition-transform duration-200 ease-out"
            style={{ transform: `scaleX(${progress})` }}
          />
        </div>
      ) : null}
      <p role="status" className="text-muted-foreground">{status}</p>
      <div className="-mx-s-5 -mb-s-5 flex justify-end gap-s-2 bg-sidebar px-s-5 py-s-4">
        <Later close={close} disabled={installing} />
        <Button disabled={installing} onClick={() => void install()}>
          <Download />
          Update and restart
        </Button>
      </div>
    </div>
  );
}
