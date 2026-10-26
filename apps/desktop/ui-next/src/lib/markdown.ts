// Discord's chat markdown, read into a tree. The message text stays plain on the wire; this module only decides how the
// text is shown. It builds data, never HTML: the renderer in components/chat/markdown.tsx turns each node into React
// elements. The module has no imports, so the unit tests can run it directly with node --test.
// Syntax follows Discord's "Markdown Text 101": bold, italic, underline, strikethrough, spoilers, headers, subtext,
// masked links, lists, inline and fenced code, block quotes, and backslash escapes.

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'url'; href: string }
  | { kind: 'code'; text: string }
  | { kind: 'link'; href: string; label: Inline[] }
  | { kind: 'em' | 'strong' | 'underline' | 'strike' | 'spoiler'; children: Inline[] };

export interface ListItem {
  inline: Inline[];
  children: Block[];
}

export type Block =
  | { kind: 'para'; inline: Inline[] }
  | { kind: 'heading'; level: 1 | 2 | 3; inline: Inline[] }
  | { kind: 'subtext'; inline: Inline[] }
  | { kind: 'quote'; blocks: Block[] }
  | { kind: 'list'; ordered: boolean; items: ListItem[] }
  | { kind: 'code'; lang: string; text: string };

const URL_RE = /\bhttps?:\/\/[^\s<>"']+/g;
const MAX_CACHE = 500;
const cache = new Map<string, Block[]>();

// ASCII punctuation, the characters a backslash can escape.
function isPunct(c: string | undefined): boolean {
  return c !== undefined && /[!-/:-@[-`{-~]/.test(c);
}

// Whether the character at `index` is preceded by an odd number of backslashes, so it is escaped.
function escaped(s: string, index: number): boolean {
  let n = 0;
  for (let k = index - 1; k >= 0 && s[k] === '\\'; k--) n++;
  return n % 2 === 1;
}

// The first unescaped `delim` at or after `from`, or -1.
function closerAt(s: string, delim: string, from: number): number {
  let j = s.indexOf(delim, from);
  while (j !== -1 && escaped(s, j)) j = s.indexOf(delim, j + 1);
  return j;
}

// Plain text, with bare links split out as links.
function textNodes(text: string, inLink: boolean): Inline[] {
  if (inLink) return [{ kind: 'text', text }];
  const out: Inline[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    if (match.index > last) out.push({ kind: 'text', text: text.slice(last, match.index) });
    out.push({ kind: 'url', href: match[0] });
    last = match.index + match[0].length;
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) });
  return out;
}

// Emphasis and the other paired markers, longest first, so `***` wins over `**` and `**` over `*`.
const PAIRS: Array<{ delim: string; make: (children: Inline[]) => Inline; word: boolean }> = [
  { delim: '***', make: (c) => ({ kind: 'strong', children: [{ kind: 'em', children: c }] }), word: false },
  { delim: '**', make: (c) => ({ kind: 'strong', children: c }), word: false },
  { delim: '__', make: (c) => ({ kind: 'underline', children: c }), word: true },
  { delim: '~~', make: (c) => ({ kind: 'strike', children: c }), word: false },
  { delim: '||', make: (c) => ({ kind: 'spoiler', children: c }), word: false },
  { delim: '*', make: (c) => ({ kind: 'em', children: c }), word: false },
  { delim: '_', make: (c) => ({ kind: 'em', children: c }), word: true },
];

const WORD_CHAR = /[\p{L}\p{N}]/u;

// Inline markup. `inLink` is set inside a masked link's label, where bare links are not split out again.
export function parseInline(s: string, inLink = false): Inline[] {
  const out: Inline[] = [];
  let buf = '';
  const flush = () => {
    if (buf) out.push(...textNodes(buf, inLink));
    buf = '';
  };
  let i = 0;
  while (i < s.length) {
    const c = s[i];

    // A backslash before punctuation shows that character, with no markup.
    if (c === '\\' && isPunct(s[i + 1])) {
      buf += s[i + 1];
      i += 2;
      continue;
    }

    // Inline code: a run of backticks closed by a run of the same length. Nothing inside it is read as markup.
    if (c === '`') {
      let n = 0;
      while (s[i + n] === '`') n++;
      const fence = '`'.repeat(n);
      let j = s.indexOf(fence, i + n);
      while (j !== -1 && s[j + n] === '`') j = s.indexOf(fence, j + n + 1);
      if (j !== -1) {
        flush();
        out.push({ kind: 'code', text: s.slice(i + n, j) });
        i = j + n;
      } else {
        buf += fence;
        i += n;
      }
      continue;
    }

    // Masked link: [label](https://…). Only http and https addresses make a link; anything else is shown as written.
    if (c === '[') {
      const close = s.indexOf(']', i + 1);
      if (close !== -1 && s[close + 1] === '(') {
        const end = s.indexOf(')', close + 2);
        const href = end === -1 ? '' : s.slice(close + 2, end).trim();
        const label = s.slice(i + 1, close);
        if (label.trim() && safeHref(href)) {
          flush();
          out.push({ kind: 'link', href, label: parseInline(label, true) });
          i = end + 1;
          continue;
        }
      }
      buf += c;
      i++;
      continue;
    }

    // Paired markers. The content is read as markup too, so `**_both_**` and `~~**x**~~` nest.
    let matched = false;
    for (const { delim, make, word } of PAIRS) {
      if (!s.startsWith(delim, i)) continue;
      const start = i + delim.length;
      const close = closerAt(s, delim, start);
      if (close <= start) continue;
      // A single marker opens only before a non-space and closes only after one.
      if (delim.length === 1 || delim === '__') {
        if (/\s/.test(s[start]) || /\s/.test(s[close - 1])) continue;
      }
      // Underscores inside a word (snake_case) are not markup.
      if (word && (WORD_CHAR.test(s[i - 1] ?? '') || WORD_CHAR.test(s[close + delim.length] ?? ''))) continue;
      flush();
      out.push(make(parseInline(s.slice(start, close), inLink)));
      i = close + delim.length;
      matched = true;
      break;
    }
    if (matched) continue;

    buf += c;
    i++;
  }
  flush();
  return out;
}

// The address when it is http or https, else null.
export function safeHref(href: string): string | null {
  return /^https?:\/\/[^\s<>"']+$/i.test(href) ? href : null;
}

const HEADING = /^(#{1,3}) (.*)$/;
const SUBTEXT = /^-# (.*)$/;
const ITEM = /^( *)([-*]|\d+\.) (.*)$/;
const FENCE = /^```(\S*)\s*$/;

interface Item {
  indent: number;
  ordered: boolean;
  text: string;
}

function itemOf(line: string): Item | null {
  const m = ITEM.exec(line);
  if (!m) return null;
  return { indent: m[1].length, ordered: /\d/.test(m[2]), text: m[3] };
}

// A list starting at `start`. Items indented under an item become that item's children.
function parseList(lines: string[], start: number): { block: Block; next: number } {
  const first = itemOf(lines[start]) as Item;
  const base = first.indent;
  const items: ListItem[] = [];
  let i = start;
  while (i < lines.length) {
    const item = itemOf(lines[i]);
    if (!item || item.indent < base || (item.indent === base && item.ordered !== first.ordered)) break;
    if (item.indent > base) break;
    const entry: ListItem = { inline: parseInline(item.text), children: [] };
    i++;
    const child = i < lines.length ? itemOf(lines[i]) : null;
    if (child && child.indent > base) {
      const nested = parseList(lines, i);
      entry.children.push(nested.block);
      i = nested.next;
    }
    items.push(entry);
  }
  return { block: { kind: 'list', ordered: first.ordered, items }, next: i };
}

// Whether a line starts a block of its own, so a paragraph ends before it.
function startsBlock(line: string): boolean {
  return HEADING.test(line) || SUBTEXT.test(line) || line.startsWith('>') || itemOf(line) !== null || FENCE.test(line);
}

function parseLines(lines: string[]): Block[] {
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    if (line.trim() === '') {
      i++;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const close = lines.indexOf('```', i + 1);
      if (close !== -1) {
        blocks.push({ kind: 'code', lang: fence[1], text: lines.slice(i + 1, close).join('\n') });
        i = close + 1;
        continue;
      }
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({ kind: 'heading', level: heading[1].length as 1 | 2 | 3, inline: parseInline(heading[2]) });
      i++;
      continue;
    }

    const subtext = SUBTEXT.exec(line);
    if (subtext) {
      blocks.push({ kind: 'subtext', inline: parseInline(subtext[1]) });
      i++;
      continue;
    }

    if (line.startsWith('>')) {
      // A quote is the run of `>` lines. The `>>> ` form quotes the rest of the message, in the caller.
      const quoted: string[] = [];
      while (i < lines.length && lines[i].startsWith('>')) {
        quoted.push(lines[i].replace(/^> ?|^>/, ''));
        i++;
      }
      blocks.push({ kind: 'quote', blocks: parseLines(quoted) });
      continue;
    }

    if (itemOf(line)) {
      const list = parseList(lines, i);
      blocks.push(list.block);
      i = list.next;
      continue;
    }

    // A paragraph runs until a blank line or the start of another block.
    const para: string[] = [line];
    i++;
    while (i < lines.length && lines[i].trim() !== '' && !startsBlock(lines[i])) {
      para.push(lines[i]);
      i++;
    }
    blocks.push({ kind: 'para', inline: parseInline(para.join('\n')) });
  }
  return blocks;
}

// The blocks of a message. A `>>> ` line quotes everything after it.
export function parseBlocks(text: string): Block[] {
  const lines = text.split('\n');
  const at = lines.findIndex((l) => l.startsWith('>>> ') || l === '>>>');
  if (at === -1) return parseLines(lines);
  const before = parseLines(lines.slice(0, at));
  const rest = [lines[at].slice(4), ...lines.slice(at + 1)];
  return [...before, { kind: 'quote', blocks: parseLines(rest) }];
}

// The blocks of a message, cached: the same text is read once.
export function parseMarkdown(text: string): Block[] {
  const hit = cache.get(text);
  if (hit) return hit;
  const blocks = parseBlocks(text);
  if (cache.size >= MAX_CACHE) cache.clear();
  cache.set(text, blocks);
  return blocks;
}

// The text of inline nodes, with the markers gone.
export function inlinePlain(nodes: Inline[]): string {
  return nodes
    .map((n) => {
      switch (n.kind) {
        case 'text':
          return n.text;
        case 'url':
          return n.href;
        case 'code':
          return n.text;
        case 'link':
          return inlinePlain(n.label);
        default:
          return inlinePlain(n.children);
      }
    })
    .join('');
}

function blockPlain(blocks: Block[]): string[] {
  return blocks.flatMap((b) => {
    switch (b.kind) {
      case 'para':
      case 'heading':
      case 'subtext':
        return [inlinePlain(b.inline)];
      case 'quote':
        return blockPlain(b.blocks);
      case 'list':
        return b.items.flatMap((item) => [inlinePlain(item.inline), ...blockPlain(item.children)]);
      case 'code':
        return [b.text];
    }
  });
}

// The message as plain text: markers gone, link labels and code kept. For notifications and the sidebar preview.
export function markdownPlain(text: string): string {
  return blockPlain(parseMarkdown(text)).join('\n');
}

// Wraps the selection [start, end) in `marker`, or puts an empty pair at the caret. The selection stays on the same
// words, so a second press wraps them again.
export function wrapSelection(text: string, start: number, end: number, marker: string): { text: string; start: number; end: number } {
  const before = text.slice(0, start);
  const inside = text.slice(start, end);
  const after = text.slice(end);
  return {
    text: `${before}${marker}${inside}${marker}${after}`,
    start: start + marker.length,
    end: end + marker.length,
  };
}

// The host of a link whose label is itself a link, when it differs from its address: a link that says one site and goes
// to another should be confirmed before it opens. Null when the label is not a link or both go to the same host.
export function mismatchedHost(label: string, href: string): string | null {
  const shown = label.trim();
  if (!/^https?:\/\//i.test(shown) && !/^[\w-]+(\.[\w-]+)+(\/\S*)?$/.test(shown)) return null;
  let target: URL;
  let text: URL;
  try {
    target = new URL(href);
    text = new URL(/^https?:\/\//i.test(shown) ? shown : `https://${shown}`);
  } catch {
    return null;
  }
  return text.host === target.host ? null : text.host;
}
