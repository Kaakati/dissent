// Typing indicators: the row above the composer, the signals the composer sends, and the names the engine
// reports. The engine throttles and expires signals too, so this throttle only keeps keystrokes cheap.
import { api } from './api.js';
import { el } from './ui.js';
import { typingParts } from './typing-words.js';

const SEND_EVERY_MS = 3000;

// The row is always rendered at one height, so the list does not jump when someone starts or stops typing.
export const typingRow = el('div', { class: 'typing-row', 'aria-live': 'polite' });

// Last names the engine reported for each target, and the target on screen.
const seen = new Map();
let shown = null;
const lastSent = new Map();

// A target is { kind: 'channel', communityId, channelId } | { kind: 'dm', peer } | { kind: 'group', groupId }.
export function typingKey(target) {
  if (!target) return null;
  return [target.kind, target.communityId ?? '', target.channelId ?? '', target.peer ?? '', target.groupId ?? ''].join('|');
}

function paint(names) {
  if (names.length === 0) {
    typingRow.replaceChildren();
    return;
  }
  const words = typingParts(names).map((part) => (typeof part === 'string' ? part : el('strong', { text: part.name })));
  typingRow.replaceChildren(
    el('span', { class: 'typing-dots', 'aria-hidden': 'true' }, el('i'), el('i'), el('i')),
    el('span', { class: 'typing-text' }, ...words),
  );
}

// Shows who is typing in the conversation on screen. `target` is null when no conversation is open.
export function showTyping(target) {
  if (typingKey(target) === typingKey(shown)) return;
  shown = target;
  const key = typingKey(target);
  if (!target) {
    paint([]);
    return;
  }
  if (seen.has(key)) {
    paint(seen.get(key));
    return;
  }
  paint([]);
  api.typingIn(target)
    .then((names) => {
      seen.set(key, names);
      if (typingKey(shown) === key) paint(names);
    })
    .catch(() => {});
}

// Handles `dissent://typing-changed`: the engine's names for one target.
export function applyTyping({ target, names }) {
  const key = typingKey(target);
  seen.set(key, names);
  if (typingKey(shown) === key) paint(names);
}

// Called on each change to the composer. Empty text means the composer was cleared, so the typing stops.
export function typed(target, text) {
  if (!target) return;
  if (text.trim() === '') {
    stopTyping(target);
    return;
  }
  const key = typingKey(target);
  const now = Date.now();
  if (now - (lastSent.get(key) ?? -Infinity) < SEND_EVERY_MS) return;
  lastSent.set(key, now);
  api.typing(target).catch(() => {});
}

// Called when a message is sent, or the composer is cleared.
export function stopTyping(target) {
  if (!target) return;
  lastSent.delete(typingKey(target));
  api.stopTyping(target).catch(() => {});
}
