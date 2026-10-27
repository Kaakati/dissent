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
 * Converts the emoticons in `text`. With `keepEscapes` (while typing), an escaped emoticon keeps its backslash, so the
 * send can still see it. Without it (the send), the backslash goes and the emoticon is sent as typed.
 */
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
