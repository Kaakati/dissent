// Avatar decorations: art drawn around a person's picture. A built-in is named by its id and matched against the art this
// build ships (the list is decorations.json, bundled by the plugin in vite.config.ts), so nothing is fetched and an id
// this build does not ship shows nothing. An upload is a `dimg` URL of the copy the engine decoded and checked, with a
// still frame the engine made. Every API response passes through noteImages(), which notes decorations here.
import builtins from 'virtual:decorations';

export type BuiltinDecoration = { id: string; name: string; url: string };
// What the engine sends for a person: a built-in's id, or the two URLs of an upload.
export type DecorationField = { builtin?: string | null; decoration_image?: string | null; decoration_poster?: string | null };
// What an overlay draws: the looping art, and its still frame where the engine made one.
export type Decoration = { url: string; still: string | null };

// One of this device's past uploads, offered again in the picker.
export type RecentDecoration = { id: string; decoration_image: string; decoration_poster?: string | null };

export const BUILTIN_DECORATIONS: readonly BuiltinDecoration[] = builtins;
const byId = new Map(builtins.map((d) => [d.id, d]));

export const builtinDecoration = (id: string): Decoration | null => {
  const art = byId.get(id);
  return art ? { url: art.url, still: null } : null;
};

const decorations = new Map<string, Decoration>(); // root id -> what to draw

function shown(field: DecorationField | null | undefined): Decoration | null {
  if (!field) return null;
  if (field.builtin) return builtinDecoration(field.builtin);
  return field.decoration_image ? { url: field.decoration_image, still: field.decoration_poster ?? null } : null;
}

export function setDecoration(id: string, field: unknown): void {
  const next = shown(field as DecorationField | null);
  if (next) decorations.set(id, next);
  else decorations.delete(id);
}

export function decorationOf(id: string | null | undefined): Decoration | null {
  return id ? decorations.get(id) ?? null : null;
}

// A built-in's still frame: its first frame, drawn once per piece of art, from the app's own origin so the canvas stays readable. Null when the browser cannot draw it, and a row then shows no art until it is hovered.
const stills = new Map<string, Promise<string | null>>();
export function stillOf(url: string): Promise<string | null> {
  let still = stills.get(url);
  if (!still) {
    still = (async () => {
      try {
        const img = new Image();
        img.src = url;
        await img.decode();
        const canvas = document.createElement('canvas');
        canvas.width = img.naturalWidth;
        canvas.height = img.naturalHeight;
        canvas.getContext('2d')?.drawImage(img, 0, 0);
        const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve));
        return blob ? URL.createObjectURL(blob) : null;
      } catch {
        return null;
      }
    })();
    stills.set(url, still);
  }
  return still;
}

// The formats and the largest size the engine takes. It decodes the file to be sure, so this only spares a slow refusal.
export const DECORATION_TYPES = ['image/webp', 'image/gif', 'image/png', 'image/apng'];
export const MAX_DECORATION_FILE = 2 * 1024 * 1024;
