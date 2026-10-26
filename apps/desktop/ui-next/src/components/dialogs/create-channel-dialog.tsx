// Create Channel, laid out as Discord's: a channel type, a name that follows the type, a Private Channel switch (not
// built yet, so disabled) and two footer buttons. Text is the default unless `kind` is 'voice'. Ported from
// openCreateChannelDialog in the old UI's views/dialogs.js. Names are cleaned as they are typed.
import { type KeyboardEvent, useState } from 'react';
import { Hash, Lock, Volume2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { createChannel } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';

// Text channel names follow Discord: lowercase, spaces become hyphens, other characters are dropped. Voice names are
// kept as typed.
export const cleanChannelName = (raw: string, voice: boolean): string =>
  voice ? raw : raw.toLowerCase().replace(/\s+/g, '-').replace(/[^\p{L}\p{N}_-]/gu, '');

// The dialog's own layout: a 480 px panel surface, 16 px paddings, and a two-column footer on the panel surface.
const FRAME =
  'w-[min(480px,92vw)] rounded-lg border-border bg-panel [&_[data-slot=dialog-header]]:px-s-4 [&_[data-slot=dialog-header]]:pt-s-4 [&_[data-slot=dialog-body]]:gap-s-4 [&_[data-slot=dialog-body]]:p-s-4';

function KindRow({
  value,
  checked,
  onPick,
  icon: Icon,
  title,
  description,
}: {
  value: 'text' | 'voice';
  checked: boolean;
  onPick: (value: 'text' | 'voice') => void;
  icon: typeof Hash;
  title: string;
  description: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-s-3 rounded-lg border border-border p-s-3 hover:bg-elevated">
      <input
        type="radio"
        name="channel-kind"
        value={value}
        checked={checked}
        onChange={() => onPick(value)}
        className="mt-0.5 size-5 shrink-0 cursor-pointer appearance-none rounded-full border-2 border-faint checked:border-accent checked:bg-[radial-gradient(circle,var(--text-strong)_0_4px,var(--accent)_5px)]"
      />
      <span className="flex min-w-0 flex-col gap-1">
        <span className="inline-flex items-center gap-s-2 text-body leading-[19px] font-semibold text-text-strong">
          <Icon className="size-5" aria-hidden="true" />
          {title}
        </span>
        <span className="text-sm leading-[17px] text-muted-foreground">{description}</span>
      </span>
    </label>
  );
}

function CreateChannelBody({ communityId, kind, close }: { communityId: string; kind: 'text' | 'voice'; close: () => void }) {
  const [voice, setVoice] = useState(kind === 'voice');
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const cleaned = cleanChannelName(name, voice).trim();

  const submit = async () => {
    const next = cleanChannelName(name, voice).trim();
    if (!next || busy) return;
    setBusy(true);
    const ok = await createChannel(communityId, next, { voice });
    if (ok) close();
    else setBusy(false);
  };

  // Switching the type re-cleans the name, as the current UI does.
  const pick = (value: 'text' | 'voice') => {
    setVoice(value === 'voice');
    setName((n) => cleanChannelName(n, value === 'voice'));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    }
  };

  const NameIcon = voice ? Volume2 : Hash;
  return (
    <>
      <section className="flex flex-col gap-s-2">
        <h3 id="channel-type-label" className="text-sm leading-[19px] font-semibold text-text-strong">Channel Type</h3>
        <div role="radiogroup" aria-labelledby="channel-type-label" className="flex flex-col gap-s-2">
          <KindRow value="text" checked={!voice} onPick={pick} icon={Hash} title="Text" description="Send messages, images, GIFs, emoji, opinions, and puns" />
          <KindRow value="voice" checked={voice} onPick={pick} icon={Volume2} title="Voice" description="Hang out together with voice" />
        </div>
      </section>
      <section className="flex flex-col gap-s-2">
        <h3 className="text-sm leading-[19px] font-semibold text-text-strong">Channel Name</h3>
        <div className="relative flex items-center">
          <NameIcon className="pointer-events-none absolute left-s-3 size-4 text-muted-foreground" aria-hidden="true" />
          <Input
            placeholder="new-channel"
            maxLength={64}
            aria-label="Channel name"
            autoComplete="off"
            autoFocus
            value={name}
            onChange={(e) => setName(cleanChannelName(e.target.value, voice))}
            onKeyDown={onKeyDown}
            className="h-10 pl-9"
          />
        </div>
      </section>
      <section className="flex flex-col gap-1">
        <div className="flex items-center justify-between gap-s-3">
          <span className="inline-flex items-center gap-s-2 text-body leading-[19px] font-semibold text-text-strong">
            <Lock className="size-5" aria-hidden="true" />
            Private Channel
          </span>
          <label title="Coming soon" className="relative inline-flex h-6 w-10 shrink-0 cursor-not-allowed opacity-60">
            <input type="checkbox" role="switch" className="sr-only" disabled aria-label="Private Channel" aria-description="Coming soon" />
            <span aria-hidden="true" className="relative h-full w-full rounded-full bg-elevated shadow-[inset_0_0_0_1px_var(--border)] after:absolute after:top-1 after:left-1 after:size-4 after:rounded-full after:bg-muted" />
          </label>
        </div>
        <p className="text-sm leading-[17px] text-muted-foreground">Only selected members and roles will be able to view this channel.</p>
      </section>
      <DialogFooter className="mt-0 -mx-s-4 -mb-s-4 grid grid-cols-2 bg-panel px-s-4 py-s-4 [&_button]:w-full">
        <Button variant="secondary" onClick={close}>Cancel</Button>
        <Button onClick={() => void submit()} disabled={!cleaned || busy}>Create Channel</Button>
      </DialogFooter>
    </>
  );
}

export function openCreateChannelDialog(communityId: string, kind: 'text' | 'voice' = 'text'): void {
  const serverName = useShell.getState().community?.id === communityId ? useShell.getState().community?.name : '';
  openDialog({
    title: 'Create Channel',
    sub: serverName ? `in ${serverName}` : null,
    className: FRAME,
    content: (close) => <CreateChannelBody communityId={communityId} kind={kind} close={close} />,
  });
}
