//! Generic at-rest encryption for local secrets.
//!
//! A [`Vault`] holds one 32-byte data key. The key comes from the OS credential store
//! ([`Vault::open`]) or from a passphrase ([`Vault::from_passphrase`]). Sealed blobs use
//! XChaCha20-Poly1305 with a random 24-byte nonce:
//!
//! `version (1 byte) || nonce (24 bytes) || ciphertext and tag`
//!
//! The caller passes an `aad` label that names the file purpose, for example
//! `b"dissent/identity.bin"`. A blob sealed under one label does not open under another,
//! so a ciphertext can't be moved between files.

use crate::{IdentityError, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use std::fmt;
use zeroize::Zeroize;

const VAULT_VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// Argon2id parameters for the passphrase fallback: 64 MiB, 3 passes, 1 lane.
const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
const ARGON2_PASSES: u32 = 3;
const ARGON2_LANES: u32 = 1;

/// Holds the data key. The key is wiped when the vault is dropped. It has no `Clone`
/// or `Debug` output that would expose it.
pub struct Vault {
    key: [u8; 32],
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault(..)")
    }
}

fn keychain_err(e: impl fmt::Display) -> IdentityError {
    IdentityError::Keychain(e.to_string())
}

impl Vault {
    /// Fetch the data key for `app`/`profile` from the OS credential store, or create one.
    ///
    /// Backends: Windows Credential Manager, macOS Keychain, and the Secret Service on Linux.
    /// Returns [`IdentityError::Keychain`] when no store is available or access is refused.
    /// Callers may then use [`Vault::from_passphrase`]. This never falls back to plaintext.
    pub fn open(app: &str, profile: &str) -> Result<Vault> {
        let entry = keyring::Entry::new(app, profile).map_err(keychain_err)?;
        match entry.get_secret() {
            Ok(mut bytes) => {
                let key = <[u8; 32]>::try_from(bytes.as_slice());
                bytes.zeroize();
                let key = key.map_err(|_| keychain_err("stored data key has the wrong length"))?;
                Ok(Vault { key })
            }
            Err(keyring::Error::NoEntry) => {
                let vault = Vault { key: rand::random() };
                entry.set_secret(&vault.key).map_err(keychain_err)?;
                Ok(vault)
            }
            Err(e) => Err(keychain_err(e)),
        }
    }

    /// Derive the data key from a passphrase with Argon2id (64 MiB, 3 passes, 1 lane).
    /// The same passphrase and salt always give the same key. Store the 16-byte salt next
    /// to the data it protects.
    pub fn from_passphrase(passphrase: &str, salt: &[u8; 16]) -> Result<Vault> {
        let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_PASSES, ARGON2_LANES, Some(32))
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        // Build the vault first so its key is wiped if derivation fails.
        let mut vault = Vault { key: [0; 32] };
        argon
            .hash_password_into(passphrase.as_bytes(), salt, &mut vault.key)
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        Ok(vault)
    }

    /// A vault with a random key that lives only in memory. For tests.
    pub fn ephemeral_for_tests() -> Vault {
        Vault { key: rand::random() }
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.key).into())
    }

    /// Encrypt `plaintext` and bind it to `aad`. Output: version || nonce || ciphertext.
    ///
    /// Panics only if the plaintext is beyond the cipher's size limit (about 256 GiB).
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let nonce: [u8; NONCE_LEN] = rand::random();
        let ct = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
            .expect("XChaCha20-Poly1305 fails only past its 256 GiB limit");
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
        out.push(VAULT_VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    /// Reverse [`Vault::seal`]. Fails if the key, `aad` or any byte differs from sealing.
    pub fn open_sealed(&self, aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 1 + NONCE_LEN + TAG_LEN || data[0] != VAULT_VERSION {
            return Err(IdentityError::Crypto("not a sealed blob".into()));
        }
        let (nonce, ct) = data[1..].split_at(NONCE_LEN);
        self.cipher()
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| IdentityError::Crypto("sealed data did not authenticate".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AAD: &[u8] = b"dissent/identity.bin";

    #[test]
    fn seal_and_open_roundtrip() {
        let v = Vault::ephemeral_for_tests();
        let sealed = v.seal(AAD, b"secret seed bytes");
        assert_eq!(sealed[0], VAULT_VERSION);
        assert_eq!(v.open_sealed(AAD, &sealed).unwrap(), b"secret seed bytes");
    }

    #[test]
    fn wrong_aad_fails() {
        let v = Vault::ephemeral_for_tests();
        let sealed = v.seal(AAD, b"data");
        assert!(v.open_sealed(b"dissent/other.bin", &sealed).is_err());
    }

    #[test]
    fn tampered_byte_fails() {
        let v = Vault::ephemeral_for_tests();
        let sealed = v.seal(AAD, b"data");
        for i in [1, NONCE_LEN, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[i] ^= 0x01;
            assert!(v.open_sealed(AAD, &bad).is_err(), "flip at {i} must fail");
        }
    }

    #[test]
    fn passphrase_is_deterministic_for_same_salt() {
        let salt = [7u8; 16];
        let a = Vault::from_passphrase("correct horse", &salt).unwrap();
        let b = Vault::from_passphrase("correct horse", &salt).unwrap();
        let sealed = a.seal(AAD, b"x");
        assert_eq!(b.open_sealed(AAD, &sealed).unwrap(), b"x");
        let other_salt = Vault::from_passphrase("correct horse", &[8u8; 16]).unwrap();
        assert!(other_salt.open_sealed(AAD, &sealed).is_err());
    }

    #[test]
    fn ephemeral_vaults_differ() {
        let a = Vault::ephemeral_for_tests();
        let b = Vault::ephemeral_for_tests();
        let sealed = a.seal(AAD, b"x");
        assert!(b.open_sealed(AAD, &sealed).is_err());
    }

    /// Touches the real OS credential store, so it is ignored by default.
    /// Run with: `cargo test -p dissent-identity keychain -- --ignored`
    #[test]
    #[ignore]
    fn keychain_roundtrip_uses_same_key() {
        let app = "dissent-identity-test";
        let profile = format!("test-{:016x}", rand::random::<u64>());
        let first = Vault::open(app, &profile).unwrap();
        let sealed = first.seal(AAD, b"x");
        let second = Vault::open(app, &profile).unwrap();
        assert_eq!(second.open_sealed(AAD, &sealed).unwrap(), b"x");
        keyring::Entry::new(app, &profile).unwrap().delete_credential().unwrap();
    }
}
