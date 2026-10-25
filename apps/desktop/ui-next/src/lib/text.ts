// Text in messages is built as nodes, never parsed as HTML. Only http and https links become anchors.

const URL_RE = /\bhttps?:\/\/[^\s<>"']+/g;

export type TextPart = { text: string; href?: undefined } | { text: string; href: string };

// Splits text into plain runs and links, in order.
export function linkParts(text: string): TextPart[] {
  const out: TextPart[] = [];
  let last = 0;
  for (const match of String(text).matchAll(URL_RE)) {
    if (match.index > last) out.push({ text: text.slice(last, match.index) });
    out.push({ text: match[0], href: match[0] });
    last = match.index + match[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
