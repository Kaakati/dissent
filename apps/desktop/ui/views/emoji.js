// Emoji picker for reactions: a curated grid drawn with the system emoji font, a name search, arrow-key navigation.
// No emoji fonts or lists are fetched; this list is the whole set.
import { el } from '../ui.js';

const COLUMNS = 8;
export const EMOJI = [
  ['👍', 'thumbs up like yes'], ['👎', 'thumbs down no'], ['❤️', 'heart love'], ['😂', 'joy laugh lol'],
  ['😮', 'open mouth wow surprised'], ['😢', 'cry sad tear'], ['😡', 'angry rage mad'], ['🎉', 'party tada celebrate'],
  ['🔥', 'fire hot lit'], ['👀', 'eyes looking'], ['✅', 'check done yes'], ['❌', 'cross no wrong'],
  ['🙏', 'pray thanks please'], ['👏', 'clap applause'], ['💯', 'hundred perfect'], ['🤔', 'thinking hmm'],
  ['😍', 'heart eyes love'], ['😅', 'sweat smile phew'], ['🙌', 'raised hands hooray'], ['🚀', 'rocket ship launch'],
  ['⭐', 'star favourite'], ['💡', 'bulb idea'], ['🐛', 'bug'], ['☕', 'coffee tea'],
  ['🍕', 'pizza food'], ['🎂', 'cake birthday'], ['🤝', 'handshake deal'], ['👋', 'wave hello bye'],
  ['😎', 'sunglasses cool'], ['🤷', 'shrug dunno'], ['🫡', 'salute ack'], ['✨', 'sparkles shiny'],
];

let open = null; // { node, opener }

export function closeEmojiPicker() {
  if (!open) return;
  const { node, opener } = open;
  open = null;
  node.remove();
  document.removeEventListener('pointerdown', onOutside, true);
  if (opener?.isConnected) opener.focus();
}

function onOutside(e) {
  if (open && !open.node.contains(e.target)) closeEmojiPicker();
}

// Opens the picker next to `anchor`. onPick(emoji) is called once, after the picker closes.
export function openEmojiPicker(anchor, onPick) {
  closeEmojiPicker();
  const opener = anchor ?? document.activeElement;
  const pick = (emoji) => {
    closeEmojiPicker();
    onPick(emoji);
  };
  const search = el('input', { type: 'search', class: 'emoji-search', placeholder: 'Search reactions', 'aria-label': 'Search reactions', autocomplete: 'off' });
  const grid = el('div', { class: 'emoji-grid', role: 'group', 'aria-label': 'Reactions' });
  const node = el('div', { class: 'emoji-picker', role: 'dialog', 'aria-label': 'Add reaction' }, search, grid);

  const cells = () => [...grid.querySelectorAll('.emoji-cell')];
  const fill = () => {
    const q = search.value.trim().toLowerCase();
    const matches = EMOJI.filter(([e, names]) => !q || names.includes(q) || e === q);
    grid.replaceChildren(...(matches.length
      ? matches.map(([emoji, names], i) => el('button', {
        class: 'emoji-cell',
        tabindex: i === 0 ? '0' : '-1',
        'aria-label': names.split(' ')[0],
        title: names.split(' ').slice(0, 2).join(' '),
        text: emoji,
        onclick: () => pick(emoji),
      }))
      : [el('p', { class: 'emoji-empty muted', text: 'No matching emoji.' })]));
  };
  search.addEventListener('input', fill);
  search.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      cells()[0]?.focus();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const first = cells()[0];
      if (first) pick(first.textContent);
    }
  });
  grid.addEventListener('keydown', (e) => {
    const all = cells();
    const i = all.indexOf(document.activeElement);
    if (i < 0) return;
    const moves = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLUMNS, ArrowUp: -COLUMNS, Home: -i, End: all.length - 1 - i };
    if (!(e.key in moves)) return;
    e.preventDefault();
    const next = i + moves[e.key];
    if (e.key === 'ArrowUp' && next < 0) {
      search.focus();
      return;
    }
    const target = all[Math.max(0, Math.min(all.length - 1, next))];
    all.forEach((c) => c.setAttribute('tabindex', c === target ? '0' : '-1'));
    target.focus();
  });
  node.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeEmojiPicker();
    }
  });

  fill();
  document.body.append(node);
  place(node, anchor);
  open = { node, opener };
  document.addEventListener('pointerdown', onOutside, true);
  search.focus();
}

// Below the anchor when there is room, otherwise above it, and never off the window's edges.
function place(node, anchor) {
  const a = anchor.getBoundingClientRect();
  const r = node.getBoundingClientRect();
  const left = Math.max(8, Math.min(a.right - r.width, window.innerWidth - r.width - 8));
  const below = a.bottom + 8;
  const top = below + r.height <= window.innerHeight - 8 ? below : Math.max(8, a.top - 8 - r.height);
  node.style.setProperty('left', `${Math.round(left)}px`);
  node.style.setProperty('top', `${Math.round(top)}px`);
}
