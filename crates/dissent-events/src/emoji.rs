//! Reaction emoji. A reaction is one emoji or emoji sequence, not free text.
//!
//! After NFC the string must be 1 to 8 Unicode scalar values, with no control
//! characters or whitespace. Every scalar must be non-ASCII, with one exception:
//! a keycap sequence (`#` `*` or `0`-`9`, then U+FE0F, then U+20E3) is allowed
//! and consists of exactly those three scalars. ZWJ sequences, flags, skin tones
//! and variation selectors pass.

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub const MAX_EMOJI_SCALARS: usize = 8;

const VARIATION_SELECTOR_16: char = '\u{FE0F}';
const COMBINING_ENCLOSING_KEYCAP: char = '\u{20E3}';

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum EmojiError {
    #[error("emoji must be 1 to {MAX_EMOJI_SCALARS} characters, got {0}")]
    Length(usize),
    #[error("character {0:?} is not allowed in a reaction")]
    InvalidChar(char),
}

fn is_keycap(c: &[char]) -> bool {
    matches!(c, [base, VARIATION_SELECTOR_16, COMBINING_ENCLOSING_KEYCAP]
        if matches!(base, '#' | '*' | '0'..='9'))
}

/// Validate a reaction and return its NFC form, which is the lookup key.
pub fn normalize_emoji(s: &str) -> Result<String, EmojiError> {
    let nfc: String = s.nfc().collect();
    let scalars: Vec<char> = nfc.chars().collect();
    let n = scalars.len();
    if !(1..=MAX_EMOJI_SCALARS).contains(&n) {
        return Err(EmojiError::Length(n));
    }
    if let Some(c) = scalars.iter().find(|c| c.is_control() || c.is_whitespace()) {
        return Err(EmojiError::InvalidChar(*c));
    }
    let keycap = is_keycap(&scalars);
    for (i, c) in scalars.iter().enumerate() {
        if c.is_ascii() && !(keycap && i == 0) {
            return Err(EmojiError::InvalidChar(*c));
        }
    }
    Ok(nfc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_emoji_sequences() {
        for ok in [
            "👍",
            "❤\u{FE0F}",
            "👨\u{200D}👩\u{200D}👧\u{200D}👦",
            "🇸🇦",
            "👍🏽",
            "🏳\u{FE0F}\u{200D}🌈",
            "é",
        ] {
            assert!(normalize_emoji(ok).is_ok(), "{ok:?} should pass");
        }
    }

    #[test]
    fn nfc_is_the_key() {
        // "e" plus combining acute normalizes to the precomposed letter.
        assert_eq!(normalize_emoji("e\u{0301}").unwrap(), "\u{00E9}");
    }

    #[test]
    fn rejects_text_whitespace_controls_and_length() {
        assert_eq!(normalize_emoji(""), Err(EmojiError::Length(0)));
        assert_eq!(normalize_emoji("a"), Err(EmojiError::InvalidChar('a')));
        assert_eq!(normalize_emoji("7"), Err(EmojiError::InvalidChar('7')));
        assert_eq!(normalize_emoji("👍 "), Err(EmojiError::InvalidChar(' ')));
        assert_eq!(normalize_emoji("\n"), Err(EmojiError::InvalidChar('\n')));
        assert_eq!(
            normalize_emoji("\u{0007}"),
            Err(EmojiError::InvalidChar('\u{0007}'))
        );
        assert_eq!(normalize_emoji(&"👍".repeat(9)), Err(EmojiError::Length(9)));
        assert!(normalize_emoji(&"👍".repeat(8)).is_ok());
    }

    #[test]
    fn rejects_bare_ascii_punctuation() {
        assert_eq!(normalize_emoji("#"), Err(EmojiError::InvalidChar('#')));
        assert_eq!(normalize_emoji("!"), Err(EmojiError::InvalidChar('!')));
        assert_eq!(normalize_emoji("👍!"), Err(EmojiError::InvalidChar('!')));
    }

    #[test]
    fn keycap_sequences_are_the_only_ascii() {
        for ok in [
            "#\u{FE0F}\u{20E3}",
            "1\u{FE0F}\u{20E3}",
            "*\u{FE0F}\u{20E3}",
            "0\u{FE0F}\u{20E3}",
        ] {
            assert!(normalize_emoji(ok).is_ok(), "{ok:?} should pass");
        }
        // Only #, * and 0-9 make keycaps.
        assert_eq!(
            normalize_emoji("a\u{FE0F}\u{20E3}"),
            Err(EmojiError::InvalidChar('a'))
        );
        // A keycap base without the full sequence is bare ASCII.
        assert_eq!(
            normalize_emoji("#\u{20E3}"),
            Err(EmojiError::InvalidChar('#'))
        );
        assert_eq!(
            normalize_emoji("#\u{FE0F}"),
            Err(EmojiError::InvalidChar('#'))
        );
        // A keycap cannot be mixed with other ASCII.
        assert_eq!(
            normalize_emoji("👍#\u{FE0F}\u{20E3}"),
            Err(EmojiError::InvalidChar('#'))
        );
    }
}
