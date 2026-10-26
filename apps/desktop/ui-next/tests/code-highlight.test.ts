// Unit tests for the code highlighter's pure parts: the language names and the reading of highlight.js's output. Run with
// `npm run test:unit`. The grammars themselves load in the browser, so they are covered by the driver.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { languageOf, parseHighlighted } from '../src/lib/code-highlight.ts';

test('a fence word maps to its grammar, with the common spellings', () => {
  assert.equal(languageOf('ts'), 'typescript');
  assert.equal(languageOf('Rust'), 'rust');
  assert.equal(languageOf('c++'), 'cpp');
  assert.equal(languageOf('toml'), 'ini', 'this highlight.js has no TOML grammar; INI is the nearest');
  assert.equal(languageOf('brainfuck'), null, 'an unknown language stays plain');
  assert.equal(languageOf(''), null);
});

test('highlight.js spans become a tree, and the text between them is decoded', () => {
  assert.deepEqual(parseHighlighted('<span class="hljs-keyword">let</span> x = &lt;a&gt; &amp; &quot;q&quot;'), [
    { cls: 'hljs-keyword', children: ['let'] },
    ' x = <a> & "q"',
  ]);
  assert.deepEqual(parseHighlighted('<span class="hljs-string">"a<span class="hljs-number">1</span>"</span>'), [
    { cls: 'hljs-string', children: ['"a', { cls: 'hljs-number', children: ['1'] }, '"'] },
  ]);
});

test('a span with several highlight.js classes is one span', () => {
  assert.deepEqual(parseHighlighted('<span class="hljs-title function_">main</span>'), [{ cls: 'hljs-title function_', children: ['main'] }]);
});

test('only highlight.js classes survive; any other markup is text', () => {
  assert.deepEqual(parseHighlighted('<img src=x onerror=alert(1)>'), ['<img src=x onerror=alert(1)>']);
  assert.deepEqual(parseHighlighted('<span class="evil" onclick="x">hi</span>'), ['<span class="evil" onclick="x">hi']);
  assert.deepEqual(parseHighlighted('<span class="hljs-keyword a{b}">x</span>'), ['x']);
});
