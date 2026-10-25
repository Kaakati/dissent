// Servers: create one, or join one with an invite link. Each button runs its action and closes on success.
// Ported from openServerDialog in ui/views/dialogs.js.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { createCommunity, joinCommunity } from '@/lib/actions';
import { openDialog } from '@/lib/dialog';
import { cn } from '@/lib/utils';

type Tab = 'create' | 'join';

function ServerBody({ close }: { close: () => void }) {
  const [tab, setTab] = useState<Tab>('create');
  const [name, setName] = useState('');
  const [invite, setInvite] = useState('');
  const [busy, setBusy] = useState(false);

  const create = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    setBusy(true);
    const summary = await createCommunity(trimmed);
    setBusy(false);
    if (summary) close();
  };

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
        <div className="flex flex-col gap-s-4">
          <p className="text-sm text-muted-foreground">You become the owner. A "general" channel is created for you.</p>
          <Input placeholder="Server name" maxLength={64} aria-label="Server name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter' && !e.nativeEvent.isComposing) void create(); }} />
          <DialogFooter>
            <Button onClick={() => void create()} disabled={busy}>Create server</Button>
          </DialogFooter>
        </div>
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
