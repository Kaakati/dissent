// An address with a copy button and, optionally, its QR code. Port of the old UI's views/address-card.js.
// The short form shows on screen; the title and the copy button carry the full address.
import { Copy } from 'lucide-react';
import { addressOf, shortAddress } from '@/lib/address';
import { copyText } from '@/lib/clipboard';
import { qrMatrix } from '@/lib/qr';
import { toast } from '@/lib/toasts';

type Kind = 'user' | 'server' | 'device';

function QrCode({ text, label }: { text: string; label: string }) {
  // qrMatrix throws when the text is too long for a QR code (the long contact link). Show no code then.
  let m: boolean[][];
  try {
    m = qrMatrix(text);
  } catch {
    return null;
  }
  const quiet = 4;
  const size = m.length + quiet * 2;
  let d = '';
  m.forEach((row, y) => row.forEach((on, x) => { if (on) d += `M${x + quiet} ${y + quiet}h1v1h-1z`; }));
  return (
    <div className="qr-code size-[192px] shrink-0 overflow-hidden rounded-r-2" role="img" aria-label={label} title={text}>
      <svg viewBox={`0 0 ${size} ${size}`} width="100%" height="100%" shapeRendering="crispEdges" aria-hidden="true">
        <rect width={size} height={size} fill="#fff" />
        <path d={d} fill="#000" />
      </svg>
    </div>
  );
}

export function AddressCard({ kind, id, label, qr = false }: { kind: Kind; id?: string | null; label?: string; qr?: boolean }) {
  const full: string = id ? addressOf(kind, id) : '';
  const what = kind[0].toUpperCase() + kind.slice(1);
  const copy = async () => {
    if (await copyText(full)) toast(`${what} address copied`, 'info');
    else toast('Could not copy the address', 'error');
  };
  return (
    <div className="address-card flex flex-col gap-2">
      <div className="id-row flex items-center gap-2">
        <span className="id-label text-muted-foreground">{label ?? `${what} address`}</span>
        <code className="mono id-value" title={full}>{shortAddress(full) || '—'}</code>
        {full ? (
          <button type="button" className="icon-btn" aria-label={`Copy ${kind} address`} title={`Copy ${kind} address`} onClick={copy}>
            <Copy className="size-4" aria-hidden="true" />
          </button>
        ) : null}
      </div>
      {qr && full ? <QrCode text={full} label={`QR code of the ${kind} address`} /> : null}
      {qr && full ? <p className="text-muted-foreground text-sm">Scan to verify this address</p> : null}
    </div>
  );
}
