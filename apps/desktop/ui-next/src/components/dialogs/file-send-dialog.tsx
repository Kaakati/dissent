// Share a file (the FileSend board). Opens after the native picker; the targets are the open channel (with the seeder
// choice) and the open direct conversation or another member, peer to peer. The copy is honest: files stay with the
// people who receive them, and the limit is 10 GB. Ported from openFileSendDialog in ui/views/dialogs.js.
import { useState } from 'react';
import { CircleAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { sendFiles, type FileTarget } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';

export const MAX_SHARE_BYTES = 10 * 1024 ** 3;

export interface ShareFile {
  name: string;
  size: number;
  path: string;
}

interface ShareTarget {
  key: 'channel' | 'direct';
  title: string;
  sub: string;
  initials: string;
  target: FileTarget | { kind: 'dm'; peer: string };
  steps: string[];
  via: string;
  // The far end of the route: a member count for a channel (plus the seeder when it keeps a copy), else a name.
  members?: number;
  end: string;
  send: string;
}

function fileExt(name: string): string {
  return name.includes('.') ? (name.split('.').pop() ?? '') : '';
}

export function formatShareSize(n: number): string {
  const b = Number(n) || 0;
  if (b >= 1024 ** 3) return `${(b / 1024 ** 3).toFixed(1)} GB`;
  if (b >= 1024 ** 2) return `${(b / 1024 ** 2).toFixed(1)} MB`;
  if (b >= 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${b} B`;
}

// The places a file can go from where the user is: the open channel, and a direct target (the open DM, or the other
// member of the open server when no DM is open).
function targetsNow(): ShareTarget[] {
  const s = useShell.getState();
  const out: ShareTarget[] = [];
  if (s.community && s.channelId) {
    const channel = s.community.channels.find((c) => c.id === s.channelId);
    if (channel) {
      const members = s.community.members.filter((m) => !m.banned).length;
      const community = s.community;
      out.push({
        key: 'channel',
        title: `#${channel.name}`,
        sub: `${community.name} · ${members} members`,
        initials: community.name.split(/\s+/).map((w) => w[0]).join('').slice(0, 2).toUpperCase(),
        target: { kind: 'channel', communityId: community.id, channelId: channel.id, seed: true },
        steps: [
          'Your device locks the file with a new key and keeps the encrypted copy.',
          `The key goes out in a sealed message only the ${members} members of #${channel.name} can open.`,
          'Members download from you and from each other at the same time, and from the seeder when you’re offline.',
        ],
        via: 'Members share the load',
        members,
        end: 'Members',
        send: `Share to #${channel.name}`,
      });
    }
  }
  const direct = (name: string, peer: string): ShareTarget => ({
    key: 'direct',
    title: `${name}, directly`,
    sub: 'Peer to peer · just the two of you',
    initials: name.slice(0, 2).toUpperCase(),
    target: { kind: 'dm', peer },
    steps: [
      `Your device locks the file with a new key. Only ${name} gets the key.`,
      'It goes straight from your device to theirs. If your networks block that, a relay passes along the encrypted bytes.',
      `If ${name} is offline, it waits on your device. Keep Dissent running until it’s delivered.`,
    ],
    via: 'Direct, or a relay if needed',
    end: name,
    send: `Send to ${name}`,
  });
  if (s.route.kind === 'home' && s.peer && !s.group) {
    out.push(direct(s.peerName || 'them', s.peer));
  } else if (s.community && s.channelId) {
    const other = s.community.members.find((m) => !m.banned && m.root_id !== s.me?.root_id);
    if (other) out.push(direct(other.display_name, other.root_id));
  }
  return out;
}

function ShareBody({ files, close }: { files: ShareFile[]; close: () => void }) {
  const [targets] = useState(targetsNow);
  const [current, setCurrent] = useState<ShareTarget | undefined>(targets[0]);
  const [seed, setSeed] = useState(true);
  const [note, setNote] = useState('');
  const [sending, setSending] = useState(false);
  const total = files.reduce((n, f) => n + (Number(f.size) || 0), 0);
  const over = files.some((f) => Number(f.size) > MAX_SHARE_BYTES);
  if (!current) return null;

  const end = current.members === undefined ? current.end : `${current.members} members${seed && current.key === 'channel' ? ' + seeder' : ''}`;

  const send = async () => {
    setSending(true);
    const target: FileTarget = current.target.kind === 'channel' ? { ...current.target, seed } : current.target;
    const ok = await sendFiles(files.map((f) => f.path), note.trim() || null, target);
    if (ok) close();
    else setSending(false);
  };

  return (
    <>
      <div className="flex items-center gap-s-3 rounded-lg bg-sidebar p-s-3">
        <span className="grid size-10 shrink-0 place-items-center rounded-md bg-accent text-sm font-bold text-white uppercase">
          {files.length === 1 ? fileExt(files[0].name).toUpperCase() || 'FILE' : String(files.length)}
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-ui font-semibold text-text-strong">{files.length === 1 ? files[0].name : `${files.length} files`}</span>
          <span className="text-meta text-muted-foreground">{formatShareSize(total) + (files.length === 1 ? '' : ' in total')}</span>
        </span>
        <span
          className={cn(
            'inline-flex h-[22px] shrink-0 items-center rounded-pill px-s-2 text-meta font-semibold whitespace-nowrap',
            over ? 'bg-danger-tint text-danger-text' : 'bg-success-tint text-success-text',
          )}
        >
          {over ? 'Over the 10 GB limit' : 'Under the 10 GB limit'}
        </span>
      </div>
      <span className="text-sm font-semibold tracking-[.02em] text-muted-foreground uppercase">Send to</span>
      <div role="radiogroup" aria-label="Send to" className="grid grid-cols-2 gap-s-2">
        {targets.map((t) => {
          const on = t === current;
          return (
            <button
              key={t.key}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => setCurrent(t)}
              className={cn(
                'flex min-h-16 items-center gap-s-3 rounded-lg border px-s-3 py-2.5 text-left',
                on ? 'border-accent-text bg-accent-tint' : 'border-white/10 bg-sidebar',
              )}
            >
              <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-elevated text-meta font-bold text-text-strong">{t.initials}</span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-ui font-semibold text-text-strong">{t.title}</span>
                <span className="truncate text-meta text-muted-foreground">{t.sub}</span>
              </span>
            </button>
          );
        })}
      </div>
      <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-panel p-s-3">
        <span className="text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">How it travels</span>
        <div className="flex items-center gap-2.5 text-sm">
          <span className="rounded-md bg-elevated px-2 py-1 font-medium text-text-strong">You</span>
          <span className="h-px flex-1 bg-border-strong" />
          <span className="text-muted-foreground">{current.via}</span>
          <span className="h-px flex-1 bg-border-strong" />
          <span className="rounded-md bg-elevated px-2 py-1 font-medium text-text-strong">{end}</span>
        </div>
        <ol className="flex flex-col gap-1.5">
          {current.steps.map((s, i) => (
            <li key={i} className="flex min-h-12 items-center gap-3.5 rounded-lg px-s-3 text-body text-muted-foreground">
              <span className="grid size-7 shrink-0 place-items-center rounded-full text-tiny font-bold text-muted-foreground shadow-[inset_0_0_0_2px_var(--faint)]">{i + 1}</span>
              <span>{s}</span>
            </li>
          ))}
        </ol>
      </div>
      {current.key === 'channel' ? (
        <SeedRow checked={seed} onChange={setSeed} />
      ) : null}
      <div className="flex items-start gap-s-2 rounded-lg bg-warn-tint p-s-3 text-sm text-warn">
        <CircleAlert className="mt-px size-4 shrink-0" aria-hidden="true" />
        <span>Anyone who receives this file keeps access to it, even if they’re removed later.</span>
      </div>
      <Input placeholder="Add a message (optional)" aria-label="Add a message" value={note} onChange={(e) => setNote(e.target.value)} />
      <DialogFooter>
        <Button variant="outline" onClick={close}>Cancel</Button>
        <Button onClick={() => void send()} disabled={over || sending}>{current.send}</Button>
      </DialogFooter>
    </>
  );
}

function SeedRow({ checked, onChange }: { checked: boolean; onChange: (on: boolean) => void }) {
  return (
    <label className="flex cursor-pointer items-start gap-3 rounded-lg bg-sidebar p-s-3">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} aria-label="Let the Shop seeder keep an encrypted copy" className="mt-px size-5 m-0 shrink-0 accent-accent" />
      <span className="flex flex-col">
        <span className="text-ui font-semibold text-text-strong">Let the seeder keep an encrypted copy</span>
        <span className="text-meta text-muted-foreground">Members can still download it when you’re offline. The seeder never gets the key, so it can’t open the file.</span>
      </span>
    </label>
  );
}

// Opens nothing when there is nowhere to send the files.
export function openFileSendDialog(files: ShareFile[]): void {
  if (targetsNow().length === 0) return;
  openDialog({ title: 'Share a file', content: (close) => <ShareBody files={files} close={close} /> });
}
