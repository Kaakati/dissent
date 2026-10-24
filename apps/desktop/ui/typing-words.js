// The words under the message list: who is typing. Pure, with no DOM, so the keyboard driver can check
// every wording on the page. Names come oldest first. The parts are plain strings, and `{ name }` for a
// name, which the row sets in bold.

export function typingParts(names) {
  const n = names.length;
  const name = (i) => ({ name: names[i] });
  if (n === 0) return [];
  if (n === 1) return [name(0), ' is typing…'];
  if (n === 2) return [name(0), ' and ', name(1), ' are typing…'];
  if (n === 3) return [name(0), ', ', name(1), ' and ', name(2), ' are typing…'];
  // Up to four people are named; beyond that, three names and a count.
  if (n === 4) return [name(0), ', ', name(1), ', ', name(2), ' and ', name(3), ' are typing…'];
  return [name(0), ', ', name(1), ', ', name(2), ` and ${n - 3} more are typing…`];
}

// The sentence as plain text, for the accessible name and for checks.
export const typingText = (names) =>
  typingParts(names)
    .map((part) => (typeof part === 'string' ? part : part.name))
    .join('');
