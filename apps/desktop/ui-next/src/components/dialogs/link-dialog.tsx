// A link that is made on open, with a Copy button: invite links, the contact link and a group's invite link. Ported
// from openLinkDialog in ui/views/dialogs.js. Copy shows a check mark for a moment, as flashCopied did.
import { type ReactNode, useEffect, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { copyText } from '@/lib/clipboard';
import { openDialog } from '@/lib/dialog';
import { toast } from '@/lib/toasts';

// Extra content under the link: a node, or a function of the link once it has loaded (a QR code of it, say).
type Extra = ReactNode | ((link: string | null) => ReactNode);

interface LinkBodyProps {
  description: string;
  label: string;
  load: () => Promise<string | undefined>;
  extra?: Extra;
}

function LinkBody({ description, label, load, extra }: LinkBodyProps) {
  const [link, setLink] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let live = true;
    void load().then((value) => {
      if (!live) return;
      if (value) setLink(value);
      else setFailed(true);
    });
    return () => {
      live = false;
    };
    // The link is made once, when the dialog opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    if (!link) return;
    if (await copyText(link)) {
      toast('Copied to clipboard', 'info');
      setCopied(true);
    }
  };

  return (
    <>
      <p className="text-sm text-muted-foreground">{description}</p>
      <div className="flex gap-s-2">
        <Input
          readOnly
          aria-label={label}
          placeholder={failed ? 'Could not create link' : 'Creating link…'}
          value={link ?? ''}
          className="font-mono text-meta"
        />
        <Button onClick={() => void copy()} disabled={!link}>
          {copied ? <Check /> : <Copy />}
          {copied ? 'Copied' : 'Copy'}
        </Button>
      </div>
      {typeof extra === 'function' ? extra(link) : extra}
    </>
  );
}

export function openLinkDialog({
  title,
  description,
  label = title,
  load,
  extra,
}: {
  title: string;
  description: string;
  label?: string;
  load: () => Promise<string | undefined>;
  extra?: Extra;
}): void {
  openDialog({
    title,
    content: () => <LinkBody description={description} label={label} load={load} extra={extra} />,
  });
}
