// Create Channel, laid out as Discord's: a channel type, a name that follows the type, a Private Channel switch with
// its member and role picker, and two footer buttons. Text is the default unless `kind` is 'voice'; only text channels
// can be private. Ported from openCreateChannelDialog in the old UI's views/dialogs.js. Names are cleaned as they are typed.
import { type KeyboardEvent, useState } from 'react';
import { Hash, Lock, Volume2 } from 'lucide-react';
import { AccessPicker, type AccessValue, PrivateSwitch } from '@/components/channel-settings/access-picker';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { createChannel } from '@/lib/actions';
import { has } from '@/lib/features';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import type { Member } from '@/lib/types';

const NO_MEMBERS: Member[] = [];

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

function CreateChannelBody({ communityId, kind, category, close }: { communityId: string; kind: 'text' | 'voice'; category?: string; close: () => void }) {
  const [voice, setVoice] = useState(kind === 'voice');
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [isPrivate, setPrivate] = useState(false);
  const [access, setAccess] = useState<AccessValue>({ roles: [], members: [] });
  const members = useShell((s) => (s.community?.id === communityId ? s.community.members : NO_MEMBERS));
  const cleaned = cleanChannelName(name, voice).trim();
  const privateText = isPrivate && !voice;

  const submit = async () => {
    const next = cleanChannelName(name, voice).trim();
    if (!next || busy) return;
    setBusy(true);
    const ok = await createChannel(communityId, next, { voice, access: privateText ? access : undefined, category });
    if (ok) close();
    else setBusy(false);
  };

  // Switching the type re-cleans the name, as the current UI does.
  const pick = (value: 'text' | 'voice') => {
    setVoice(value === 'voice');
    setName((n) => cleanChannelName(n, value === 'voice'));
    if (value === 'voice') setPrivate(false);
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
      {has('private_channels') ? (
        <section className="flex flex-col gap-1">
          <div className="flex items-center justify-between gap-s-3">
            <span className="inline-flex items-center gap-s-2 text-body leading-[19px] font-semibold text-text-strong">
              <Lock className="size-5" aria-hidden="true" />
              Private Channel
            </span>
            <PrivateSwitch checked={privateText} onChange={setPrivate} disabled={voice} />
          </div>
          <p className="text-sm leading-[17px] text-muted-foreground">Only selected members and roles will be able to view this channel.</p>
          {privateText ? (
            <div className="mt-s-2">
              <AccessPicker members={members} value={access} onChange={setAccess} />
            </div>
          ) : null}
        </section>
      ) : null}
      <DialogFooter className="mt-0 -mx-s-4 -mb-s-4 grid grid-cols-2 bg-panel px-s-4 py-s-4 [&_button]:w-full">
        <Button variant="secondary" onClick={close}>Cancel</Button>
        <Button onClick={() => void submit()} disabled={!cleaned || busy}>Create Channel</Button>
      </DialogFooter>
    </>
  );
}

// With `category`, the new channel goes at the end of that category in the server's layout.
export function openCreateChannelDialog(communityId: string, kind: 'text' | 'voice' = 'text', category?: string): void {
  const community = useShell.getState().community?.id === communityId ? useShell.getState().community : null;
  const found = category ? community?.layout?.find((e) => e.kind === 'category' && e.id === category) : undefined;
  const categoryName = found?.kind === 'category' ? found.name : undefined;
  const where = categoryName ?? community?.name;
  openDialog({
    title: 'Create Channel',
    sub: where ? `in ${where}` : null,
    className: FRAME,
    content: (close) => <CreateChannelBody communityId={communityId} kind={kind} category={category} close={close} />,
  });
}
