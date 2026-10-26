// Servers: create one, or join one with an invite link. Each button runs its action and closes on success.
// Create opens "Customize Your Server": a name, prefilled with the user's, and an optional icon. Create makes the server,
// sets the icon when one was chosen, and opens it. Ported from openServerDialog in the old UI's views/dialogs.js; the
// customize step is the Discord reference's, without its guidelines line.
import { useEffect, useRef, useState } from 'react';
import { Camera, Plus } from 'lucide-react';
import { chooseImage } from '@/components/settings/avatar-editor';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { createCommunity, joinCommunity, setCommunityIcon } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';

type Tab = 'create' | 'join';

// The icon the user picked, as the crop editor encodes it (base64 of an image).
function IconTile({ icon, onPick }: { icon: string | null; onPick: () => void }) {
  return (
    <button
      type="button"
      aria-label={icon ? 'Change server icon' : 'Upload a server icon'}
      onClick={onPick}
      className="relative grid size-24 shrink-0 place-items-center overflow-visible rounded-full border-2 border-dashed border-border-strong bg-elevated text-muted-foreground hover:text-text-strong focus-visible:outline-2 focus-visible:outline-ring"
    >
      {icon ? (
        <img src={`data:image/webp;base64,${icon}`} alt="" className="size-full rounded-full object-cover" draggable={false} />
      ) : (
        <Camera className="size-8" aria-hidden="true" />
      )}
      <span aria-hidden="true" className="absolute right-0 bottom-0 grid size-7 place-items-center rounded-full border-2 border-chat bg-accent text-white">
        <Plus className="size-4" />
      </span>
    </button>
  );
}

// "Customize Your Server": the name is selected when the form opens, so typing replaces it.
function CustomizeForm({ onBack, close }: { onBack: () => void; close: () => void }) {
  const me = useShell((s) => s.me);
  const [name, setName] = useState(`${me?.display_name || 'My'}'s server`);
  const [icon, setIcon] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameField = useRef<HTMLInputElement>(null);
  useEffect(() => {
    nameField.current?.select();
  }, []);

  const create = async () => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    const summary = await createCommunity(trimmed);
    if (summary && icon) await setCommunityIcon(summary.id, icon);
    setBusy(false);
    if (summary) close();
  };

  return (
    <div className="flex flex-col gap-s-4">
      <div className="flex flex-col gap-s-1">
        <h2 className="text-head font-bold text-text-strong">Customize Your Server</h2>
        <p className="text-sm text-muted-foreground">Give your new server a personality with a name and an icon. You can always change it later.</p>
      </div>
      <div className="flex justify-center">
        <IconTile icon={icon} onPick={() => chooseImage({ shape: 'rounded', onApply: (bytes) => setIcon(bytes) })} />
      </div>
      <label className="flex flex-col gap-s-2 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">
        Server Name <span aria-hidden="true" className="text-danger-text">*</span>
        <Input
          ref={nameField}
          aria-label="Server Name"
          required
          maxLength={64}
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.nativeEvent.isComposing) void create();
          }}
          className="normal-case tracking-normal"
        />
      </label>
      <DialogFooter>
        <Button variant="secondary" onClick={onBack}>
          Back
        </Button>
        <Button onClick={() => void create()} disabled={busy || !name.trim()} className="bg-accent text-white hover:bg-accent-hover">
          Create
        </Button>
      </DialogFooter>
    </div>
  );
}

function ServerBody({ close }: { close: () => void }) {
  const [tab, setTab] = useState<Tab>('create');
  const [invite, setInvite] = useState('');
  const [busy, setBusy] = useState(false);

  const join = async () => {
    const link = invite.trim();
    if (!link) return;
    setBusy(true);
    const summary = await joinCommunity(link);
    setBusy(false);
    if (summary) close();
  };

  return (
    <>
      <div role="tablist" className="flex gap-s-4 border-b border-border">
        {(['create', 'join'] as const).map((key) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            onClick={() => setTab(key)}
            className={cn(
              'h-10 text-body font-medium text-muted-foreground hover:text-text',
              tab === key && 'text-text-strong shadow-[inset_0_-2px_var(--accent-text)]',
            )}
          >
            {key === 'create' ? 'Create server' : 'Join server'}
          </button>
        ))}
      </div>
      {tab === 'create' ? (
        <CustomizeForm onBack={() => setTab('join')} close={close} />
      ) : (
        <div className="flex flex-col gap-s-4">
          <p className="text-sm text-muted-foreground">Paste an invite link from someone in the server.</p>
          <Textarea rows={3} placeholder="dissent://invite/…" aria-label="Invite link" value={invite} onChange={(e) => setInvite(e.target.value)} />
          <DialogFooter>
            <Button onClick={() => void join()} disabled={busy}>Join server</Button>
          </DialogFooter>
        </div>
      )}
    </>
  );
}

export function openServerDialog(): void {
  openDialog({ title: 'Servers', content: (close) => <ServerBody close={close} /> });
}
