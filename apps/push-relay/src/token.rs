//! Wake tokens and secrets. The relay keeps only hashes of both, so a dump of its storage cannot
//! wake a device or delete a registration.

use crate::PushError;
use data_encoding::BASE32_NOPAD;
use ring::rand::{SecureRandom, SystemRandom};

/// Bytes in a wake token. 128 bits, random.
pub const TOKEN_BYTES: usize = 16;
/// Bytes in a delete secret.
pub const SECRET_BYTES: usize = 32;

/// `N` random bytes from the system generator.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], PushError> {
    let mut out = [0u8; N];
    SystemRandom::new()
        .fill(&mut out)
        .map_err(|_| PushError::Random)?;
    Ok(out)
}

/// Lowercase base32 without padding, the encoding the rest of Dissent uses.
pub fn encode(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes).to_ascii_lowercase()
}

/// Parses a wake token from its text form. `None` if it is not a valid token.
pub fn decode_token(text: &str) -> Option<[u8; TOKEN_BYTES]> {
    let raw = BASE32_NOPAD.decode(text.to_ascii_uppercase().as_bytes()).ok()?;
    <[u8; TOKEN_BYTES]>::try_from(raw.as_slice()).ok()
}

/// The key the store indexes registrations by.
pub fn hash_token(token: &[u8]) -> [u8; 32] {
    blake3::derive_key("dissent push-relay wake token v1", token)
}

/// The value the store checks a delete secret against.
pub fn hash_secret(secret: &[u8]) -> [u8; 32] {
    blake3::derive_key("dissent push-relay delete secret v1", secret)
}

/// Compares two hashes without an early exit.
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_text_round_trips() {
        let token: [u8; TOKEN_BYTES] = random_bytes().unwrap();
        let text = encode(&token);
        assert_eq!(text.len(), 26);
        assert_eq!(decode_token(&text), Some(token));
        assert_eq!(decode_token(&text.to_uppercase()), Some(token));
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        assert!(decode_token("").is_none());
        assert!(decode_token("not base32!").is_none());
        assert!(decode_token(&encode(&[0u8; 8])).is_none());
        assert!(decode_token(&encode(&[0u8; 32])).is_none());
    }

    #[test]
    fn hashes_differ_from_inputs_and_each_other() {
        let token = [7u8; TOKEN_BYTES];
        assert_ne!(hash_token(&token), hash_secret(&token));
        assert!(!ct_eq(&hash_token(&token), &hash_token(&[8u8; TOKEN_BYTES])));
        assert!(ct_eq(&hash_token(&token), &hash_token(&token)));
    }
}
