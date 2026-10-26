// Unit tests for the chat markdown parser. Run with `npm run test:unit` (node --test, no build step).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markdownPlain, mismatchedHost, parseInline, parseMarkdown, safeHref, wrapSelection } from '../src/lib/markdown.ts';

const t = (text: string) => ({ kind: 'text', text });

test('italic with * or _, and bold, bold italic, underline, strike and spoilers', () => {
  assert.deepEqual(parseInline('*a*'), [{ kind: 'em', children: [t('a')] }]);
  assert.deepEqual(parseInline('_a_'), [{ kind: 'em', children: [t('a')] }]);
  assert.deepEqual(parseInline('**b**'), [{ kind: 'strong', children: [t('b')] }]);
  assert.deepEqual(parseInline('***bi***'), [{ kind: 'strong', children: [{ kind: 'em', children: [t('bi')] }] }]);
  assert.deepEqual(parseInline('__u__'), [{ kind: 'underline', children: [t('u')] }]);
  assert.deepEqual(parseInline('~~s~~'), [{ kind: 'strike', children: [t('s')] }]);
  assert.deepEqual(parseInline('||hidden||'), [{ kind: 'spoiler', children: [t('hidden')] }]);
});

test('markers combine, and the inner text is read as markup', () => {
  assert.deepEqual(parseInline('**__both__**'), [{ kind: 'strong', children: [{ kind: 'underline', children: [t('both')] }] }]);
  assert.deepEqual(parseInline('~~**x**~~'), [{ kind: 'strike', children: [{ kind: 'strong', children: [t('x')] }] }]);
  assert.deepEqual(parseInline('a *b* c'), [t('a '), { kind: 'em', children: [t('b')] }, t(' c')]);
});

test('an underscore inside a word is not italic, and a marker with a space inside is not markup', () => {
  assert.deepEqual(parseInline('snake_case_name'), [t('snake_case_name')]);
  assert.deepEqual(parseInline('* not a list in a sentence * ok'), [t('* not a list in a sentence * ok')]);
});

test('backslash escapes show the marker as text', () => {
  assert.deepEqual(parseInline('\\*not\\*'), [t('*not*')]);
  assert.deepEqual(parseInline('\\\\'), [t('\\')]);
  assert.deepEqual(parseInline('\\a'), [t('\\a')]);
  assert.deepEqual(parseInline('\\*a* b'), [t('*a* b')]);
});

test('inline code is immune: nothing inside it is markup', () => {
  assert.deepEqual(parseInline('`**x** [y](https://a.example)`'), [{ kind: 'code', text: '**x** [y](https://a.example)' }]);
  assert.deepEqual(parseInline('``a`b``'), [{ kind: 'code', text: 'a`b' }]);
  assert.deepEqual(parseInline('`open'), [t('`open')]);
});

test('a masked link keeps its label and its address; other schemes are not links', () => {
  assert.deepEqual(parseInline('[docs](https://a.example/x)'), [
    { kind: 'link', href: 'https://a.example/x', label: [t('docs')] },
  ]);
  assert.deepEqual(parseInline('[x](javascript:alert(1))'), [t('[x](javascript:alert(1))')]);
  assert.deepEqual(parseInline('[x](data:text/html,hi)'), [t('[x](data:text/html,hi)')]);
  assert.equal(safeHref('javascript:alert(1)'), null);
  assert.equal(safeHref('HTTPS://a.example'), 'HTTPS://a.example');
});

test('bare links are split out of plain text, and not inside a masked link label', () => {
  assert.deepEqual(parseInline('see https://a.example/p now'), [
    t('see '),
    { kind: 'url', href: 'https://a.example/p' },
    t(' now'),
  ]);
  assert.deepEqual(parseInline('[https://a.example](https://b.example)'), [
    { kind: 'link', href: 'https://b.example', label: [t('https://a.example')] },
  ]);
});

test('headers, subtext and block quotes are blocks, and >>> quotes the rest of the message', () => {
  assert.deepEqual(parseMarkdown('# Big\n## Mid\n### Small'), [
    { kind: 'heading', level: 1, inline: [t('Big')] },
    { kind: 'heading', level: 2, inline: [t('Mid')] },
    { kind: 'heading', level: 3, inline: [t('Small')] },
  ]);
  assert.deepEqual(parseMarkdown('-# small print'), [{ kind: 'subtext', inline: [t('small print')] }]);
  assert.deepEqual(parseMarkdown('> quoted **bold**'), [
    { kind: 'quote', blocks: [{ kind: 'para', inline: [t('quoted '), { kind: 'strong', children: [t('bold')] }] }] },
  ]);
  assert.deepEqual(parseMarkdown('before\n>>> one\ntwo'), [
    { kind: 'para', inline: [t('before')] },
    { kind: 'quote', blocks: [{ kind: 'para', inline: [t('one\ntwo')] }] },
  ]);
});

test('lists nest by indent, and ordered and unordered items can sit together', () => {
  const blocks = parseMarkdown('- one\n  - nested\n- two\n1. first\n2. second');
  assert.deepEqual(blocks, [
    {
      kind: 'list',
      ordered: false,
      items: [
        { inline: [t('one')], children: [{ kind: 'list', ordered: false, items: [{ inline: [t('nested')], children: [] }] }] },
        { inline: [t('two')], children: [] },
      ],
    },
    {
      kind: 'list',
      ordered: true,
      items: [
        { inline: [t('first')], children: [] },
        { inline: [t('second')], children: [] },
      ],
    },
  ]);
  assert.deepEqual(parseMarkdown('* star bullet')[0], { kind: 'list', ordered: false, items: [{ inline: [t('star bullet')], children: [] }] });
});

test('fenced code keeps its language and its text, and markup inside it is not read', () => {
  assert.deepEqual(parseMarkdown('```ts\nconst a = **1**;\n```'), [{ kind: 'code', lang: 'ts', text: 'const a = **1**;' }]);
  assert.deepEqual(parseMarkdown('```\nplain\n```'), [{ kind: 'code', lang: '', text: 'plain' }]);
  // An unclosed fence is not a block: it is text.
  assert.deepEqual(parseMarkdown('```\nopen'), [{ kind: 'para', inline: [t('```\nopen')] }]);
});

test('raw HTML is text, never markup', () => {
  assert.deepEqual(parseInline('<script>alert(1)</script>'), [t('<script>alert(1)</script>')]);
  assert.deepEqual(parseMarkdown('<img src=x onerror=alert(1)>'), [{ kind: 'para', inline: [t('<img src=x onerror=alert(1)>')] }]);
});

test('a masked link that says one host and goes to another is flagged for confirmation', () => {
  assert.equal(mismatchedHost('github.com', 'https://evil.example/x'), 'github.com');
  assert.equal(mismatchedHost('https://github.com/a', 'https://github.com/b'), null);
  assert.equal(mismatchedHost('read the docs', 'https://docs.example'), null);
  assert.equal(mismatchedHost('https://a.example', 'https://b.example'), 'a.example');
});

test('plain text drops the markers and keeps labels and code', () => {
  assert.equal(markdownPlain('**bold** and [docs](https://a.example) `code`'), 'bold and docs code');
  assert.equal(markdownPlain('# Title\n- item'), 'Title\nitem');
});

test('wrapping a selection adds the marker on both sides and keeps the selection on the words', () => {
  assert.deepEqual(wrapSelection('say hi', 4, 6, '**'), { text: 'say **hi**', start: 6, end: 8 });
  assert.deepEqual(wrapSelection('hi', 1, 1, '*'), { text: 'h**i', start: 2, end: 2 });
});

test('the same text is parsed once', () => {
  assert.equal(parseMarkdown('x **y**'), parseMarkdown('x **y**'));
});
