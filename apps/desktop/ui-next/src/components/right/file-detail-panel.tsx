// The transfer details of one attachment, in the right column: the FileDetail board, or the DirectTransferDetail board for a
// transfer between this user's devices. Ported from the old UI's views/filedetail.js (fileDetailNodes, directTransferNodes); the
// boards themselves are in components/transfers/file-detail.tsx.
import { DirectTransferDetail, FileDetail } from '@/components/transfers/file-detail';
import { findAttachment, useChat } from '@/lib/chat';
import { openFileDetail } from '@/lib/right';
import type { Id } from '@/lib/types';

export function FileDetailPanel({ id }: { id: Id }) {
  const target = useChat((s) => s.target);
  // Re-read when the open conversation's messages change, so the progress and the sources stay current.
  useChat((s) => s.messages);
  useChat((s) => s.dmMessages);
  const found = findAttachment(id);
  if (!found) return null;
  const close = () => openFileDetail(null);
  if (found.direct) {
    return (
      <DirectTransferDetail
        file={{ progress: Math.min(1, Math.max(0, Number(found.progress) || 0)), transfer_id: found.transfer_id ?? null }}
        direct={found.direct}
        onClose={close}
      />
    );
  }
  return (
    <FileDetail
      file={{
        name: found.name,
        size: Number(found.size) || 0,
        progress: Math.min(1, Math.max(0, Number(found.progress) || 0)),
        transfer_id: found.transfer_id ?? null,
        fingerprint: found.fingerprint,
        shared_in: found.shared_in,
        sources: found.sources,
        pieces: found.pieces,
      }}
      scope={target?.kind === 'channel' ? 'server' : 'conversation'}
      onClose={close}
    />
  );
}
