// A QR code of a link, with a quiet zone, on white so it scans on the dark surface. Nothing is drawn when the link is
// too long for a code: the link is still there to copy. Ported from qrSvg in ui/views/dialogs.js (the box is .qr-code).
import { useMemo } from 'react';
import { qrMatrix } from '@/lib/qr';

const QUIET = 4;

export function QrCode({ text, label }: { text: string; label: string }) {
  const matrix = useMemo(() => {
    try {
      return qrMatrix(text);
    } catch {
      return null;
    }
  }, [text]);
  if (!matrix) return null;
  const n = matrix.length + QUIET * 2;
  let d = '';
  matrix.forEach((row, y) => row.forEach((on, x) => {
    if (on) d += `M${x + QUIET} ${y + QUIET}h1v1h-1z`;
  }));
  return (
    <div role="img" aria-label={label} title={text} className="my-s-3 block w-fit rounded-lg bg-white p-s-3">
      <svg viewBox={`0 0 ${n} ${n}`} shapeRendering="crispEdges" className="block size-[168px] text-black">
        <path d={d} fill="currentColor" />
      </svg>
    </div>
  );
}
