// The small dialogs with one field: claim a handle, a private note or nickname on a contact, and add a contact. Ported
// from ui/views/dialogs.js (openClaimHandleDialog, openNoteDialog, openAddContactDialog).
import { useState } from 'react';
import { AtSign, UserPlus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { isAddressToAdd } from '@/lib/address';
import { addContact, claimHandle, setNote } from '@/lib/actions';
import { toast } from '@/lib/toasts';
import { openDialog } from '@/lib/dialog';

// ---- claim a handle ----

function ClaimHandleBody({ communityId, close }: { communityId: string; close: () => void }) {
  const [handle, setHandle] = useState('');
  const [busy, setBusy] = useState(false);
  const claim = async () => {
    const value = handle.trim();
    if (!value) return;
    setBusy(true);
    const ok = await claimHandle(communityId, value);
    setBusy(false);
    if (ok) close();
  };
  return (
    <>
      <p className="text-sm text-muted-foreground">Your handle is shown next to your name in this server.</p>
      <Input placeholder="your-handle" maxLength={32} aria-label="Handle" value={handle} onChange={(e) => setHandle(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') void claim(); }} />
      <DialogFooter>
        <Button onClick={() => void claim()} disabled={busy}>
          <AtSign />
          Claim handle
        </Button>
      </DialogFooter>
    </>
  );
}

export function openClaimHandleDialog(communityId: string): void {
  openDialog({ title: 'Claim handle', content: (close) => <ClaimHandleBody communityId={communityId} close={close} /> });
}

// ---- note or nickname ----

function NoteBody({ peer, kind, close }: { peer: string; kind: 'note' | 'nickname'; close: () => void }) {
  const [text, setText] = useState('');
  const isNote = kind === 'note';
  const save = async () => {
    if (await setNote(peer, kind, text.trim())) close();
  };
  return (
    <>
      <p className="text-sm text-muted-foreground">{isNote ? 'Only visible to you.' : 'Shown in place of the name, only on this device.'}</p>
      <Textarea rows={3} aria-label={isNote ? 'Note' : 'Nickname'} value={text} onChange={(e) => setText(e.target.value)} />
      <DialogFooter>
        <Button variant="outline" onClick={close}>Cancel</Button>
        <Button onClick={() => void save()}>Save</Button>
      </DialogFooter>
    </>
  );
}

// One dialog for both kinds: a note is only for you, a nickname changes the name on this device.
export function openNoteDialog(peer: string, name: string, kind: 'note' | 'nickname'): void {
  openDialog({
    title: kind === 'note' ? `Note on ${name}` : `Nickname for ${name}`,
    content: (close) => <NoteBody peer={peer} kind={kind} close={close} />,
  });
}

// ---- add a contact ----

// An address (dsu1…) or a contact link (dissent://contact/…). The status line says "Looking them up…" for an address.
function AddContactBody({ close }: { close: () => void }) {
  const [link, setLink] = useState('');
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);

  const add = async () => {
    const value = link.trim();
    if (!value) return;
    let byAddress: boolean;
    try {
      byAddress = isAddressToAdd(value);
    } catch (err) {
      toast(err);
      return;
    }
    setBusy(true);
    setStatus(byAddress ? 'Looking them up…' : '');
    const ok = await addContact(value);
    setBusy(false);
    setStatus('');
    if (ok) close();
  };

  return (
    <>
      <p className="text-sm text-muted-foreground">Paste their address to start a direct conversation. A contact link works too.</p>
      <Textarea rows={3} placeholder="dsu1… or dissent://contact/…" aria-label="Address or contact link" value={link} onChange={(e) => setLink(e.target.value)} />
      <p role="status" className="text-sm text-muted-foreground">{status}</p>
      <DialogFooter>
        <Button onClick={() => void add()} disabled={busy}>
          <UserPlus />
          Add contact
        </Button>
      </DialogFooter>
    </>
  );
}

export function openAddContactDialog(): void {
  openDialog({ title: 'Add contact', content: (close) => <AddContactBody close={close} /> });
}
