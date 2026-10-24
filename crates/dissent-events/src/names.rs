//! Handle normalization and TR39 confusable detection.
//!
//! A handle is what a user claims inside a community (`@handle`). Two handles
//! that look alike (`paypal` and `pаypal` with a Cyrillic а) must not coexist.
//! Display names are free text and are not checked here.
//!
//! Folding is NFKC, then lowercase, then NFKC again, then trim. A handle must
//! also be 2 to 32 characters from letters, digits, `_`, `-` and `.`, use no
//! bidi or invisible characters, and satisfy the TR39 "Moderately Restrictive"
//! level. That level rejects Latin mixed with Cyrillic, for example.
//!
//! Confusables use the TR39 skeleton. The skeleton is a per-character mapping
//! from the TR39 `confusables.txt` table. It includes multi-character targets,
//! so `m` maps to `rn` and `rn` is confusable with `m`.

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use unicode_security::{RestrictionLevel, RestrictionLevelDetection};

pub const MIN_HANDLE_CHARS: usize = 2;
pub const MAX_HANDLE_CHARS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum NameError {
    #[error("handle must be {MIN_HANDLE_CHARS} to {MAX_HANDLE_CHARS} characters, got {0}")]
    Length(usize),
    #[error("character {0:?} is not allowed in a handle")]
    InvalidChar(char),
    #[error("bidi control or invisible character {0:?} is not allowed")]
    BidiOrInvisible(char),
    #[error("handle mixes scripts")]
    MixedScript,
}

/// NFKC, lowercase, NFKC again, trim. Infallible, used by every name function.
fn fold(s: &str) -> String {
    let lowered: String = s.nfkc().collect::<String>().to_lowercase();
    lowered.nfkc().collect::<String>().trim().to_string()
}

fn is_bidi_or_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn is_handle_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || matches!(c, '_' | '-' | '.')
}

/// Validate and normalize a handle claim. The result is the key used for lookups.
pub fn normalize_handle(s: &str) -> Result<String, NameError> {
    let key = fold(s);
    if let Some(c) = key.chars().find(|c| is_bidi_or_invisible(*c)) {
        return Err(NameError::BidiOrInvisible(c));
    }
    if let Some(c) = key.chars().find(|c| !is_handle_char(*c)) {
        return Err(NameError::InvalidChar(c));
    }
    let len = key.chars().count();
    if !(MIN_HANDLE_CHARS..=MAX_HANDLE_CHARS).contains(&len) {
        return Err(NameError::Length(len));
    }
    if !key
        .as_str()
        .check_restriction_level(RestrictionLevel::ModeratelyRestrictive)
    {
        return Err(NameError::MixedScript);
    }
    Ok(key)
}

/// TR39 skeleton of the folded string. Two strings with equal skeletons look alike.
pub fn skeleton(s: &str) -> String {
    unicode_security::skeleton(&fold(s)).collect()
}

/// True when `a` and `b` look alike but are not the same after folding.
pub fn confusable(a: &str, b: &str) -> bool {
    fold(a) != fold(b) && skeleton(a) == skeleton(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_width_and_whitespace() {
        assert_eq!(normalize_handle("Alice ").unwrap(), "alice");
        assert_eq!(normalize_handle("ＡＬＩＣＥ").unwrap(), "alice");
        assert_eq!(normalize_handle("\u{3000}Bob\u{00A0}").unwrap(), "bob");
    }

    #[test]
    fn rejects_bad_shapes() {
        assert_eq!(normalize_handle("a"), Err(NameError::Length(1)));
        assert_eq!(
            normalize_handle(&"a".repeat(33)),
            Err(NameError::Length(33))
        );
        assert_eq!(normalize_handle("ab cd"), Err(NameError::InvalidChar(' ')));
        assert_eq!(normalize_handle("ab@cd"), Err(NameError::InvalidChar('@')));
        assert!(normalize_handle("a_b-c.d9").is_ok());
    }

    #[test]
    fn rejects_bidi_and_invisible() {
        assert_eq!(
            normalize_handle("ali\u{202E}ce"),
            Err(NameError::BidiOrInvisible('\u{202E}'))
        );
        assert_eq!(
            normalize_handle("ali\u{2066}ce"),
            Err(NameError::BidiOrInvisible('\u{2066}'))
        );
        assert_eq!(
            normalize_handle("ali\u{200B}ce"),
            Err(NameError::BidiOrInvisible('\u{200B}'))
        );
        assert_eq!(
            normalize_handle("\u{FEFF}alice"),
            Err(NameError::BidiOrInvisible('\u{FEFF}'))
        );
    }

    #[test]
    fn rejects_mixed_script() {
        // Cyrillic а (U+0430) inside a Latin word.
        assert_eq!(
            normalize_handle("p\u{0430}ypal"),
            Err(NameError::MixedScript)
        );
        // A single-script Cyrillic handle is fine.
        assert!(normalize_handle("\u{0430}\u{0440}\u{0440}\u{04CF}\u{0435}").is_ok());
    }

    #[test]
    fn paypal_and_cyrillic_lookalike_are_confusable() {
        assert!(confusable("paypal", "p\u{0430}ypal"));
        assert!(!confusable("paypal", "paypal"));
        assert!(!confusable("paypal", "payroll"));
    }

    #[test]
    fn rn_and_m_are_confusable_per_tr39() {
        assert_eq!(skeleton("m"), "rn");
        assert!(confusable("mango", "rnango"));
    }

    #[test]
    fn single_script_cyrillic_lookalike_is_confusable() {
        // All-Cyrillic "сору" (U+0441 U+043E U+0440 U+0443) looks like "copy".
        assert!(confusable("copy", "\u{0441}\u{043E}\u{0440}\u{0443}"));
    }
}
