// The disclaimer (DISCLAIMER.md at the repository root), bundled into the UI at build time. Its "Last updated" date is
// what the user accepts: the app saves the date (disclaimer.json in the profile, through accept_disclaimer), and a
// bundled notice with another date is shown again at the next launch. Also opened read-only from Settings › About.
import { create } from 'zustand';
import text from '../../../../../DISCLAIMER.md?raw';
import { api } from '@/lib/api';

export const DISCLAIMER_TITLE = 'Disclaimer and Limitation of Liability';

// The date on the notice's "Last updated:" line. Empty only if the line were removed, which a driver check catches.
export const DISCLAIMER_UPDATED = /^Last updated:\s*(.+)$/m.exec(text)?.[1].trim() ?? '';

// The notice as blocks: its headings and paragraphs. The file uses no other markdown.
export type DisclaimerBlock = { kind: 'title' | 'heading' | 'paragraph'; text: string };

export const DISCLAIMER_BLOCKS: DisclaimerBlock[] = text
  .replace(/\r\n/g, '\n')
  .split(/\n{2,}/)
  .map((block) => block.trim())
  .filter(Boolean)
  .map((block): DisclaimerBlock => {
    if (block.startsWith('## ')) return { kind: 'heading', text: block.slice(3) };
    if (block.startsWith('# ')) return { kind: 'title', text: block.slice(2) };
    return { kind: 'paragraph', text: block.replace(/\n/g, ' ') };
  })
  // The title and the date are shown by the dialog itself.
  .filter((b) => b.kind !== 'title' && !b.text.startsWith('Last updated:'));

// `gate`: first launch or a changed notice, which must be accepted. `view`: reread from Settings, closable.
type Mode = 'gate' | 'view' | null;

export const useDisclaimer = create<{ mode: Mode }>(() => ({ mode: null }));

let accepted: (() => void) | null = null;

// Resolves once this notice is accepted: at once when the profile already accepted this date, else when the user
// accepts it in the dialog. A backend that cannot answer (an older build) is not asked again.
export async function requireDisclaimer(): Promise<void> {
  let saved: string | null;
  try {
    saved = await api.disclaimerAccepted();
  } catch {
    return;
  }
  if (saved === DISCLAIMER_UPDATED) return;
  await new Promise<void>((resolve) => {
    accepted = resolve;
    useDisclaimer.setState({ mode: 'gate' });
  });
}

// Saves the acceptance, then lets the launch go on. A failed save keeps the dialog, so the user can try again.
export async function acceptDisclaimer(): Promise<void> {
  await api.acceptDisclaimer(DISCLAIMER_UPDATED);
  useDisclaimer.setState({ mode: null });
  accepted?.();
  accepted = null;
}

export function openDisclaimer(): void {
  useDisclaimer.setState({ mode: 'view' });
}

export function closeDisclaimer(): void {
  if (useDisclaimer.getState().mode === 'view') useDisclaimer.setState({ mode: null });
}
