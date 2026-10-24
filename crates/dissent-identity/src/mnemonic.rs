//! BIP-39 seed phrase <-> root key seed.

use crate::{IdentityError, Result, RootKey};
use bip39::{Language, Mnemonic};

/// Encode the root key's 32-byte seed as 24 English words.
pub fn export(root: &RootKey) -> String {
    // 32 bytes is a valid BIP-39 entropy length, so this cannot fail.
    Mnemonic::from_entropy(&root.0.seed())
        .expect("32 bytes is valid BIP-39 entropy")
        .to_string()
}

/// Decode a 24-word phrase back into a root key. The checksum must match.
pub fn import(phrase: &str) -> Result<RootKey> {
    let normalized = phrase
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &normalized)
        .map_err(|e| IdentityError::Encoding(e.to_string()))?;
    let entropy = mnemonic.to_entropy();
    let seed: [u8; 32] = entropy.try_into().map_err(|v: Vec<u8>| {
        IdentityError::Encoding(format!(
            "expected 24 words (32 bytes), got {} bytes",
            v.len()
        ))
    })?;
    Ok(RootKey::from_seed(seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_24_words() {
        let root = RootKey::generate();
        let phrase = export(&root);
        assert_eq!(phrase.split(' ').count(), 24);
        assert_eq!(import(&phrase).unwrap().id(), root.id());
    }

    #[test]
    fn import_normalises_case_and_whitespace() {
        let root = RootKey::from_seed([9; 32]);
        let messy = format!("  {}  ", export(&root).to_uppercase().replace(' ', "   "));
        assert_eq!(import(&messy).unwrap().id(), root.id());
    }

    #[test]
    fn flipped_word_fails_checksum() {
        let phrase = export(&RootKey::from_seed([9; 32]));
        let mut words: Vec<&str> = phrase.split(' ').collect();
        words[0] = if words[0] == "abandon" {
            "ability"
        } else {
            "abandon"
        };
        let bad = words.join(" ");
        assert!(matches!(import(&bad), Err(IdentityError::Encoding(_))));
    }

    #[test]
    fn wrong_length_is_rejected() {
        let phrase = export(&RootKey::from_seed([9; 32]));
        let short: Vec<&str> = phrase.split(' ').take(12).collect();
        assert!(matches!(
            import(&short.join(" ")),
            Err(IdentityError::Encoding(_))
        ));
    }
}
