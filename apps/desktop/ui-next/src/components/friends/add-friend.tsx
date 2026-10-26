// The Add Friend form: paste someone's address or contact link to send a request, and share your own contact link.
// Ported from addBox() in the old UI's views/friends.js. An address shows "Looking them up…" while the request goes out.
import { useEffect, useState } from 'react';
import { Copy } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { contactLink, sendContactRequest } from '@/lib/actions';
import { copyText } from '@/lib/clipboard';
import { isAddressToAdd } from '@/lib/address';
import { toast } from '@/lib/toasts';
import { useFriendsView } from './friends-state';

export function AddFriend() {
  const setAddOpen = useFriendsView((s) => s.setAddOpen);
  const setTab = useFriendsView((s) => s.setTab);
  const [link, setLink] = useState('');
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);
  const [mine, setMine] = useState('…');

  useEffect(() => {
    void contactLink().then((value) => {
      if (value) setMine(value);
    });
  }, []);

  const send = async () => {
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
    const sent = await sendContactRequest(value);
    setBusy(false);
    setStatus('');
    if (sent) {
      setLink('');
      setAddOpen(false);
      setTab('pending');
    }
  };

  const copyMine = async () => {
    const value = await contactLink();
    if (value && (await copyText(value))) toast('Contact link copied', 'info');
  };

  return (
    <div className="flex flex-col gap-s-6">
      <div className="flex flex-col gap-s-2 pb-s-5 shadow-[inset_0_-1px_0_var(--border)]">
        <h3 className="text-body leading-[19px] font-semibold text-text-strong">Add Friend</h3>
        <p className="text-sm leading-[19px] text-muted-foreground">
          Paste someone’s address or contact link. They’ll see your request and can accept or decline it. Nothing is sent through a company server.
        </p>
        <div className="mt-s-2 flex h-[52px] items-center gap-s-2 rounded-lg border border-accent-text bg-rail pr-1.5 pl-s-4">
          <Input
            value={link}
            onChange={(e) => setLink(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void send();
            }}
            placeholder="dissent://contact/…"
            aria-label="Contact link"
            spellCheck={false}
            className="h-10 min-w-0 flex-1 border-0 bg-transparent px-0 font-mono text-ui text-text-strong focus-visible:ring-0"
          />
          <Button onClick={() => void send()} disabled={busy}>Send Friend Request</Button>
        </div>
        <p role="status" className="text-sm leading-[19px] text-muted-foreground">{status}</p>
      </div>
      <div className="flex flex-col gap-s-2">
        <h3 className="text-body leading-[19px] font-semibold text-text-strong">Share yours</h3>
        <p className="text-sm leading-[19px] text-muted-foreground">
          Send it any way you like. People who open it can request to add you, and you decide who gets in.
        </p>
        <div className="rounded-md bg-rail p-s-3 font-mono text-meta leading-4 break-all text-text">{mine}</div>
        <Button onClick={() => void copyMine()} className="self-start">
          <Copy aria-hidden="true" />
          Copy contact link
        </Button>
      </div>
    </div>
  );
}
