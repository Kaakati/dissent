// "Remove identifying details": a per-file choice made before a file is sent. With it on, the engine sends a copy without
// where, when and with what the file was made, and without names in it, under a generic name ("photo.jpg"); the file on
// this computer stays as it is. The defaults come from the engine (inspect_files): on for photos and for videos that name
// the device or place they were recorded with, off for music, other videos and other files.
//
// When a file's details cannot be removed, sending asks first: Send as is, or Cancel.
import { api } from './api';
import { confirm } from './dialog';
import type { FileInspection } from './types';

export type MediaKind = FileInspection['kind'];

// The choice as a file carries it while it waits to be sent. Undefined until the engine has answered.
export interface ScrubChoice {
  kind?: MediaKind;
  scrub?: boolean;
  scrubbable?: boolean;
}

// The engine's answer for each path. An older backend has no inspect_files: the files then follow its defaults.
export async function inspectFiles(paths: string[]): Promise<Map<string, FileInspection>> {
  if (paths.length === 0) return new Map();
  try {
    const found = await api.inspectFiles(paths);
    return new Map(found.map((f) => [f.path, f]));
  } catch {
    return new Map();
  }
}

// The choice to show for a file the engine just inspected.
export function choiceOf(found: FileInspection | undefined): ScrubChoice {
  return found ? { kind: found.kind, scrub: found.scrub_default, scrubbable: found.scrubbable } : {};
}

// The per-file choices to send, in order, after asking about each file whose details cannot be removed. Null when the
// user cancels. Undefined when no file has a choice yet (an older backend), so the engine's defaults apply.
export async function resolveScrub(files: (ScrubChoice & { name: string })[]): Promise<boolean[] | null | undefined> {
  if (files.every((f) => f.scrub === undefined)) return undefined;
  const out: boolean[] = [];
  for (const f of files) {
    if (f.scrub && f.scrubbable === false) {
      const asIs = await confirm({
        title: 'Couldn’t remove details',
        message: `Couldn't remove details from ${f.name}. Send it as is, with its details, or cancel?`,
        okLabel: 'Send as is',
      });
      if (!asIs) return null;
      out.push(false);
    } else {
      out.push(Boolean(f.scrub));
    }
  }
  return out;
}
