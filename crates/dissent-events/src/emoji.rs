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

/// Most scalar values a trait emoji may have. Long enough for the longest RGI sequences, such as a
/// couple with two skin tones or a subdivision flag.
pub const MAX_TRAIT_EMOJI_SCALARS: usize = 32;

/// Validate a server trait's emoji and return its NFC form. A trait emoji is exactly one extended
/// grapheme cluster: skin tones, ZWJ sequences, flags, tag sequences and keycaps all pass, two
/// emoji do not. Otherwise the rules are those of [`normalize_emoji`], with room for longer
/// sequences.
pub fn normalize_trait_emoji(s: &str) -> Result<String, EmojiError> {
    use unicode_segmentation::UnicodeSegmentation;
    let nfc: String = s.nfc().collect();
    let scalars: Vec<char> = nfc.chars().collect();
    let n = scalars.len();
    if !(1..=MAX_TRAIT_EMOJI_SCALARS).contains(&n) || nfc.graphemes(true).count() != 1 {
        return Err(EmojiError::Length(n));
    }
    if let Some(c) = scalars.iter().find(|c| c.is_control() || c.is_whitespace()) {
        return Err(EmojiError::InvalidChar(*c));
    }
    // A joiner, selector or tag only modifies the emoji before it, so it cannot lead.
    let first = scalars[0];
    if matches!(first, '\u{200D}' | '\u{FE0E}' | VARIATION_SELECTOR_16 | COMBINING_ENCLOSING_KEYCAP | '\u{E0000}'..='\u{E007F}') {
        return Err(EmojiError::InvalidChar(first));
    }
    let keycap = is_keycap(&scalars);
    if let Some((_, c)) = scalars.iter().enumerate().find(|(i, c)| c.is_ascii() && !(keycap && *i == 0)) {
        return Err(EmojiError::InvalidChar(*c));
    }
    Ok(nfc)
}

#[cfg(test)]
mod trait_emoji_tests {
    use super::*;

    #[test]
    fn one_grapheme_of_any_shape_passes() {
        for ok in [
            "👍",
            "👍🏽",
            "🧑\u{200D}💻",
            "❤\u{FE0F}",
            "🇸🇦",
            "🏳\u{FE0F}\u{200D}🌈",
            "👩🏽\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}👨🏻",
            "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}",
            "1\u{FE0F}\u{20E3}",
            "🛠\u{FE0F}",
        ] {
            assert_eq!(normalize_trait_emoji(ok).as_deref(), Ok(ok), "{ok:?} should pass");
        }
    }

    #[test]
    fn two_emoji_text_and_blanks_fail() {
        for bad in ["", " ", "👍👍", "🎮🎨", "A", "ab", "👍 ", "\u{200D}"] {
            assert!(normalize_trait_emoji(bad).is_err(), "{bad:?} should fail");
        }
    }
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
