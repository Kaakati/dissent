// Syntax colouring for fenced code blocks. highlight.js (pinned in package.json) is loaded the first time a block with a
// known language is shown, so the core and each grammar cost nothing at startup. Its output is HTML, so it is never put
// into the page: it is read into a tree where only `hljs-*` classes survive, and every text run stays text.
import type { HLJSApi, LanguageFn } from 'highlight.js';

// The languages a fenced block can name, with their common spellings. TOML has no grammar in this highlight.js; INI
// is the closest it has, so TOML uses it.
const ALIASES: Record<string, string> = {
  js: 'javascript', jsx: 'javascript', ts: 'typescript', tsx: 'typescript', py: 'python', golang: 'go',
  'c++': 'cpp', cc: 'cpp', 'c#': 'csharp', cs: 'csharp', yml: 'yaml', toml: 'ini', sh: 'bash', zsh: 'bash',
  console: 'bash', html: 'xml', svg: 'xml', md: 'markdown',
};

const GRAMMARS: Record<string, () => Promise<{ default: LanguageFn }>> = {
  javascript: () => import('highlight.js/lib/languages/javascript'),
  typescript: () => import('highlight.js/lib/languages/typescript'),
  rust: () => import('highlight.js/lib/languages/rust'),
  python: () => import('highlight.js/lib/languages/python'),
  go: () => import('highlight.js/lib/languages/go'),
  java: () => import('highlight.js/lib/languages/java'),
  c: () => import('highlight.js/lib/languages/c'),
  cpp: () => import('highlight.js/lib/languages/cpp'),
  csharp: () => import('highlight.js/lib/languages/csharp'),
  json: () => import('highlight.js/lib/languages/json'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
  ini: () => import('highlight.js/lib/languages/ini'),
  bash: () => import('highlight.js/lib/languages/bash'),
  shell: () => import('highlight.js/lib/languages/shell'),
  sql: () => import('highlight.js/lib/languages/sql'),
  xml: () => import('highlight.js/lib/languages/xml'),
  css: () => import('highlight.js/lib/languages/css'),
  diff: () => import('highlight.js/lib/languages/diff'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
};

// A highlighted run: plain text, or a span with its highlight.js class and what is inside it.
export type CodeNode = string | { cls: string; children: CodeNode[] };

// The canonical language name for a fence's word, or null when it is not one this module highlights.
export function languageOf(fence: string): string | null {
  const word = fence.trim().toLowerCase();
  const name = ALIASES[word] ?? word;
  return name in GRAMMARS ? name : null;
}

let core: Promise<HLJSApi> | null = null;

// The highlight.js core, loaded once.
function loadCore(): Promise<HLJSApi> {
  if (!core) {
    core = import('highlight.js/lib/core').then((m) => m.default as HLJSApi);
    core.catch(() => (core = null));
  }
  return core;
}

const ENTITIES: Record<string, string> = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#x27;': "'", '&#39;': "'" };

function decode(text: string): string {
  return text.replace(/&(amp|lt|gt|quot|#x27|#39);/g, (m) => ENTITIES[m] ?? m);
}

// The class names of a highlight.js span, when they are all plain names and the first is a `hljs-` one (highlight.js can
// give several, like `hljs-title function_`). Anything else is not a span.
function safeClasses(attr: string): string | null {
  const names = attr.trim().split(/\s+/);
  if (!names[0].startsWith('hljs-') || !names.every((n) => /^[\w-]+$/.test(n))) return null;
  return names.join(' ');
}

// Reads highlight.js's HTML into a tree. Only the `<span class="hljs-...">` tags are understood; any other tag is text
// (highlight.js escapes the code, so a `<` in the code arrives as `&lt;`).
export function parseHighlighted(html: string): CodeNode[] {
  const root: CodeNode[] = [];
  const stack: Array<{ cls: string; children: CodeNode[] }> = [];
  const into = () => (stack.length ? stack[stack.length - 1].children : root);
  const tag = /<span class="([^"]*)">|<\/span>/g;
  let last = 0;
  for (const m of html.matchAll(tag)) {
    if (m.index > last) into().push(decode(html.slice(last, m.index)));
    const cls = m[1] === undefined ? null : safeClasses(m[1]);
    if (cls) {
      const span = { cls, children: [] as CodeNode[] };
      into().push(span);
      stack.push(span);
    } else {
      stack.pop();
    }
    last = m.index + m[0].length;
  }
  if (last < html.length) into().push(decode(html.slice(last)));
  return root;
}

// The highlighted tree of `code` in `language`, or null when the language is unknown or the grammar fails to load.
export async function highlightCode(code: string, language: string): Promise<CodeNode[] | null> {
  const name = languageOf(language);
  if (!name) return null;
  try {
    const hljs = await loadCore();
    if (!hljs.getLanguage(name)) {
      const grammar = await GRAMMARS[name]();
      hljs.registerLanguage(name, grammar.default);
    }
    return parseHighlighted(hljs.highlight(code, { language: name, ignoreIllegals: true }).value);
  } catch {
    return null;
  }
}
