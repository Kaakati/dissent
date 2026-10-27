// Unit tests for the emoticon converter. Run with `npm run test:unit` (node --test, no build step).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EMOTICONS, convertEmoticons } from '../src/lib/emoticons.ts';

test('every emoticon in the table converts on its own', () => {
  for (const [emoticon, emoji] of Object.entries(EMOTICONS)) {
    assert.equal(convertEmoticons(emoticon), emoji, emoticon);
  }
});

test('whole emoticons convert at the start, after whitespace, and before punctuation or the end', () => {
  const cases: [string, string][] = [
    [':D hi', '😄 hi'],
    ['hi :P', 'hi 😛'],
    ['ok :).', 'ok 🙂.'],
    ['sad :(, really', 'sad 🙁, really'],
    ['wow :O!', 'wow 😮!'],
    ['love <3 you', 'love ❤️ you'],
    ['line one\n:D', 'line one\n😄'],
    ['angry >:( now', 'angry 😠 now'],
    ['tears :\'( here', 'tears 😢 here'],
    ['ha XD', 'ha 😆'],
    ['cool 8) yes', 'cool 😎 yes'],
    ['hmm :/ ok', 'hmm 😕 ok'],
    ['two :D :P', 'two 😄 😛'],
    ['> :D quoted', '> 😄 quoted'],
  ];
  for (const [input, expected] of cases) assert.equal(convertEmoticons(input), expected, input);
});

test('emoticons inside words, numbers, links and code stay as written', () => {
  const unchanged = [
    'meet at 10:30',
    'a:b',
    'std::path::Path',
    'x:D2',
    ':D2 is a part number',
    'see https://example.com/a',
    'go to dissent://invite/abc:D',
    'http://x.org :/path',
    'www.example.com/:D',
    'the `:D` face',
    'run `a :P b` here',
    '```\n:D\n```',
    '```js\nconst a = 1; // :)\n',
    '**:D**',
    '[:D](https://x.org)',
    'XDA developers',
    'mail me:D',
    '@maya:D',
    '#general:)',
    '<30 people',
  ];
  for (const text of unchanged) assert.equal(convertEmoticons(text), text, text);
});

test('a list number such as 8) at the start of a line stays a list', () => {
  assert.equal(convertEmoticons('7) seven\n8) eight'), '7) seven\n8) eight');
  assert.equal(convertEmoticons('that was 8)'), 'that was 😎');
});

test('a backslash keeps an emoticon as typed, and the send drops the backslash', () => {
  assert.equal(convertEmoticons('\\:D hi'), ':D hi');
  assert.equal(convertEmoticons('say \\<3'), 'say <3');
  assert.equal(convertEmoticons('say \\:D', { keepEscapes: true }), 'say \\:D');
  // While typing the backslash stays, so the send still sees it and keeps the emoticon.
  assert.equal(convertEmoticons(convertEmoticons('say \\:D :D', { keepEscapes: true })), 'say :D 😄');
});

test('converting twice changes nothing more', () => {
  const once = convertEmoticons(':D and :P and `:)`');
  assert.equal(convertEmoticons(once), once);
});
