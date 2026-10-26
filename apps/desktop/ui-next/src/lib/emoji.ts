// The reaction picker's whole set: each emoji with its search words. No emoji font or list is fetched. From views/emoji.js.
export const EMOJI: [string, string][] = [
  ['👍', 'thumbs up like yes'], ['👎', 'thumbs down no'], ['❤️', 'heart love'], ['😂', 'joy laugh lol'],
  ['😮', 'open mouth wow surprised'], ['😢', 'cry sad tear'], ['😡', 'angry rage mad'], ['🎉', 'party tada celebrate'],
  ['🔥', 'fire hot lit'], ['👀', 'eyes looking'], ['✅', 'check done yes'], ['❌', 'cross no wrong'],
  ['🙏', 'pray thanks please'], ['👏', 'clap applause'], ['💯', 'hundred perfect'], ['🤔', 'thinking hmm'],
  ['😍', 'heart eyes love'], ['😅', 'sweat smile phew'], ['🙌', 'raised hands hooray'], ['🚀', 'rocket ship launch'],
  ['⭐', 'star favourite'], ['💡', 'bulb idea'], ['🐛', 'bug'], ['☕', 'coffee tea'],
  ['🍕', 'pizza food'], ['🎂', 'cake birthday'], ['🤝', 'handshake deal'], ['👋', 'wave hello bye'],
  ['😎', 'sunglasses cool'], ['🤷', 'shrug dunno'], ['🫡', 'salute ack'], ['✨', 'sparkles shiny'],
];

// The Discord-style :shortcode: name of each emoji, for the reaction submenu and the quick row. The first ones are the
// reaction picker's; the last one is a default quick reaction.
export const SHORTCODE: Record<string, string> = {
  '👍': 'thumbsup', '👎': 'thumbsdown', '\u2764\uFE0F': 'heart', '😂': 'joy', '😮': 'open_mouth', '😢': 'cry',
  '😡': 'rage', '🎉': 'tada', '🔥': 'fire', '👀': 'eyes', '✅': 'white_check_mark', '❌': 'x', '🙏': 'pray',
  '👏': 'clap', '💯': '100', '🤔': 'thinking', '😍': 'heart_eyes', '😅': 'sweat_smile', '🙌': 'raised_hands',
  '🚀': 'rocket', '⭐': 'star', '💡': 'bulb', '🐛': 'bug', '☕': 'coffee', '🍕': 'pizza', '🎂': 'birthday',
  '🤝': 'handshake', '👋': 'wave', '😎': 'sunglasses', '🤷': 'shrug', '🫡': 'saluting_face', '✨': 'sparkles',
  '😆': 'laughing',
};

// ":thumbsup:", or the emoji itself when it has no name here.
export function shortcodeOf(emoji: string): string {
  const name = SHORTCODE[emoji];
  return name ? `:${name}:` : emoji;
}
