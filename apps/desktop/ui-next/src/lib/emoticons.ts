// Text emoticons such as :D or <3, turned into emoji as Discord does: in the composer, so the message is sent with the
// emoji and every reader sees it, older versions included. Only a whole emoticon converts: one at the start of the text
// or after whitespace, and before whitespace, punctuation or the end. Code, links and words are left as they are, and a
// backslash before an emoticon keeps it as typed.

// The emoticons and their emoji. Longer forms are tried first, so `>:(` wins over `:(` and `:-)` over `:)`.
export const EMOTICONS: Readonly<Record<string, string>> = {
  ':)': '🙂',
  ':-)': '🙂',
  ':(': '🙁',
  ':-(': '🙁',
  ':D': '😄',
  ':-D': '😄',
  ';)': '😉',
  ';-)': '😉',
  ':P': '😛',
  ':p': '😛',
  ':-P': '😛',
  ':O': '😮',
  ':o': '😮',
  ":'(": '😢',
  '<3': '❤️',
  '</3': '💔',
  ':|': '😐',
  ':/': '😕',
  XD: '😆',
  '8)': '😎',
  '>:(': '😠',
  ':*': '😘',
  '^_^': '😊',
  '-_-': '😑',
};

const BY_LENGTH = Object.keys(EMOTICONS).sort((a, b) => b.length - a.length);

// What may follow an emoticon: the end, whitespace, or punctuation that ends a sentence or a clause.
const AFTER = /[\s.,!?;:)\]}"'…]/;

// The parts of the text that are never converted: fenced code blocks, inline code, and links. An unclosed fence runs to
// the end and an unclosed backtick to the end of its line, so a code block still being typed is left alone.
function protectedRanges(text: string): [number, number][] {
  const out: [number, number][] = [];
  const fence = /```[\s\S]*?(```|$)/g;
  for (const m of text.matchAll(fence)) out.push([m.index, m.index + m[0].length]);
  const inside = (i: number) => out.some(([a, b]) => i >= a && i < b);
  const inline = /`[^`\n]*(`|$)/gm;
  for (const m of text.matchAll(inline)) if (!inside(m.index)) out.push([m.index, m.index + m[0].length]);
  const link = /\b[a-z][a-z0-9+.-]*:\/\/\S+|\bwww\.\S+/gi;
  for (const m of text.matchAll(link)) out.push([m.index, m.index + m[0].length]);
  return out;
}

// A list number such as "8) item" at the start of a line is a list, not 😎.
function isListNumber(text: string, i: number, emoticon: string): boolean {
  return emoticon === '8)' && (i === 0 || text[i - 1] === '\n') && text[i + 2] === ' ';
}

/**
 * The live conversion while typing, as Discord does it: once a whitespace character is typed, the emoticons in the text
 * before the caret become emoji. Returns the new text and caret, or `null` when nothing changes. Only an insertion
 * converts (`inserted`, from the input event's type, which also covers typing over a selection), so deleting back
 * over an emoji never turns it into something else. Escapes are kept for the save or send.
 */
export function convertWhileTyping(value: string, caret: number, inserted: boolean): { text: string; caret: number } | null {
  if (!inserted || caret === 0 || !/\s/.test(value[caret - 1])) return null;
  const head = value.slice(0, caret);
  const converted = convertEmoticons(head, { keepEscapes: true });
  if (converted === head) return null;
  return { text: converted + value.slice(caret), caret: converted.length };
}

/**
 * Converts the emoticons in `text`. With `keepEscapes` (while typing), an escaped emoticon keeps its backslash, so the
 * send can still see it. Without it (the send), the backslash goes and the emoticon is sent as typed.
 */
// Whether a change event came from typing or pasting text, not from a deletion.
export function isInsertion(event: Event): boolean {
  const type = (event as InputEvent).inputType;
  return typeof type === 'string' ? type.startsWith('insert') : true;
}

export function convertEmoticons(text: string, { keepEscapes = false }: { keepEscapes?: boolean } = {}): string {
  const ranges = protectedRanges(text);
  const isProtected = (i: number) => ranges.some(([a, b]) => i >= a && i < b);
  let out = '';
  let i = 0;
  while (i < text.length) {
    const starts = i === 0 || /\s/.test(text[i - 1]);
    const escaped = text[i] === '\\' && (i === 0 || /\s/.test(text[i - 1]));
    const at = escaped ? i + 1 : i;
    if ((starts || escaped) && !isProtected(i)) {
      const found = BY_LENGTH.find((e) => text.startsWith(e, at) && (at + e.length === text.length || AFTER.test(text[at + e.length])));
      if (found && !isListNumber(text, at, found)) {
        out += escaped ? (keepEscapes ? `\\${found}` : found) : EMOTICONS[found];
        i = at + found.length;
        continue;
      }
    }
    out += text[i];
    i += 1;
  }
  return out;
}
