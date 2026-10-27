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
use std::path::{Path, PathBuf};
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

/// One folder, one name: the same folder gives the same text however it is typed. The canonical path when the folder
/// exists, without the Windows verbatim prefix, with forward slashes and no trailing slash, and lower case on Windows.
/// Keys are named from this, so `C:\Dissent` and `c:/dissent/` open the same key.
pub fn normal_dir(dir: &Path) -> String {
    let text = plain_text(&canonical_or_parent(dir).to_string_lossy());
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The canonical path of `dir`. A folder that does not exist yet takes its parent's canonical path, so the same folder
/// gives the same text before and after it is created.
fn canonical_or_parent(dir: &Path) -> PathBuf {
    if let Ok(full) = std::fs::canonicalize(dir) {
        return full;
    }
    let parent = match dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    match (std::fs::canonicalize(parent), dir.file_name()) {
        (Ok(full), Some(name)) => full.join(name),
        _ => dir.to_path_buf(),
    }
}

/// A path as text with forward slashes and no trailing slash. The verbatim prefix goes, and a UNC share keeps its two
/// leading slashes, so `\\?\UNC\server\share\dir` and `\\server\share\dir` both give `//server/share/dir`.
fn plain_text(raw: &str) -> String {
    let unprefixed = match raw.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => raw.strip_prefix(r"\\?\").unwrap_or(raw).to_string(),
    };
    let mut text = unprefixed.replace('\\', "/");
    while text.len() > 1 && text.ends_with('/') {
        text.pop();
    }
    text
}

/// The texts an older build may have hashed for `dir`: the path as typed, with either kind of slash, with and without
/// a trailing slash, and with the drive letter in either case. A key made under one of these spellings is a candidate
/// for this folder.
pub fn spellings(dir: &Path) -> Vec<String> {
    let raw = dir.to_string_lossy().into_owned();
    let mut texts: Vec<String> = Vec::new();
    let mut add = |t: String| {
        if !texts.contains(&t) {
            texts.push(t);
        }
    };
    for base in [raw.clone(), raw.replace('\\', "/"), raw.replace('/', "\\")] {
        let bare = base.trim_end_matches(['/', '\\']).to_string();
        for slash in ["", "/", "\\"] {
            let spelled = format!("{bare}{slash}");
            add(spelled.clone());
            let mut chars: Vec<char> = spelled.chars().collect();
            if chars.len() > 1 && chars[1] == ':' && chars[0].is_ascii_alphabetic() {
                chars[0] = chars[0].to_ascii_uppercase();
                add(chars.iter().collect());
                chars[0] = chars[0].to_ascii_lowercase();
                add(chars.iter().collect());
            }
        }
    }
    texts
}

impl Vault {
    /// Fetch the data key for `app`/`profile` from the OS credential store, or create one.
    ///
    /// Backends: Windows Credential Manager, macOS Keychain, and the Secret Service on Linux.
    /// Returns [`IdentityError::Keychain`] when no store is available or access is refused.
    /// Callers may then use [`Vault::from_passphrase`]. This never falls back to plaintext.
    ///
    /// Use [`Vault::open_existing`] when sealed data is already on disk: a new key cannot open it.
    pub fn open(app: &str, profile: &str) -> Result<Vault> {
        if let Some(vault) = Self::open_existing(app, profile)? {
            return Ok(vault);
        }
        let vault = Vault {
            key: rand::random(),
        };
        keyring::Entry::new(app, profile)
            .map_err(keychain_err)?
            .set_secret(&vault.key)
            .map_err(keychain_err)?;
        Ok(vault)
    }

    /// The data key for `app`/`profile` when the store has one. Never creates a key: `None` means the entry is
    /// missing, so the caller can refuse to start rather than seal new data under a new key.
    pub fn open_existing(app: &str, profile: &str) -> Result<Option<Vault>> {
        let entry = keyring::Entry::new(app, profile).map_err(keychain_err)?;
        match entry.get_secret() {
            Ok(mut bytes) => {
                let key = <[u8; 32]>::try_from(bytes.as_slice());
                bytes.zeroize();
                let key = key.map_err(|_| keychain_err("stored data key has the wrong length"))?;
                Ok(Some(Vault { key }))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    /// Removes the key stored under `profile`, when there is one. Tests use it to clean up their own test entries.
    pub fn delete_key(app: &str, profile: &str) -> Result<()> {
        match keyring::Entry::new(app, profile).map_err(keychain_err)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }

    /// Copies the key stored under `from` over whatever `to` holds, when `from` has one. `from` is never deleted.
    /// Returns whether a key was copied. Used when the key that sealed a folder's data is under an older name.
    pub fn replace_key(app: &str, from: &str, to: &str) -> Result<bool> {
        let Some(source) = Self::open_existing(app, from)? else {
            return Ok(false);
        };
        keyring::Entry::new(app, to)
            .map_err(keychain_err)?
            .set_secret(&source.key)
            .map_err(keychain_err)?;
        Ok(true)
    }

    /// Copies the key stored under `from` to `to`, when `to` has none and `from` has one. `from` is never
    /// deleted. Returns whether a key was copied. Used when a folder gets a new name for its key.
    pub fn adopt_key(app: &str, from: &str, to: &str) -> Result<bool> {
        if Self::open_existing(app, to)?.is_some() {
            return Ok(false);
        }
        let Some(source) = Self::open_existing(app, from)? else {
            return Ok(false);
        };
        keyring::Entry::new(app, to)
            .map_err(keychain_err)?
            .set_secret(&source.key)
            .map_err(keychain_err)?;
        Ok(true)
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
        Vault {
            key: rand::random(),
        }
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.key).into())
    }
    /// A vault for one purpose, keyed with a subkey derived from this vault's key. Each file
    /// sealed under its own subkey stays separate from every other file, even if an AAD label
    /// is reused by mistake. `purpose` names the file, for example `b"dissent/mls-state/v1"`.
    pub fn subkey(&self, purpose: &[u8]) -> Vault {
        let mut key = blake3::derive_key(
            "dissent/vault-subkey/v1",
            &[&self.key[..], purpose].concat(),
        );
        let vault = Vault { key };
        key.zeroize();
        vault
    }

    /// Encrypt `plaintext` and bind it to `aad`. Output: version || nonce || ciphertext.
    ///
    /// Panics only if the plaintext is beyond the cipher's size limit (about 256 GiB).
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let nonce: [u8; NONCE_LEN] = rand::random();
        let ct = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
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
    fn unc_and_verbatim_paths_read_as_one_forward_slash_text() {
        assert_eq!(plain_text(r"\\?\UNC\server\share\dir\"), "//server/share/dir");
        assert_eq!(plain_text(r"\\server\share\dir"), "//server/share/dir");
        assert_eq!(plain_text(r"\\?\C:\Dissent\"), "C:/Dissent");
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_takes_its_parents_canonical_path() {
        let base = std::env::temp_dir().join(format!("dissent-normal-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let missing = base.join("not-yet");
        assert_eq!(normal_dir(&missing), format!("{}/not-yet", normal_dir(&base)));
        std::fs::remove_dir_all(&base).ok();
    }

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

    #[test]
    #[cfg(windows)] // `\` is a separator only on Windows; elsewhere it is a filename character
    fn one_folder_has_one_name_however_it_is_typed() {
        let back = normal_dir(Path::new(r"C:\Users\Me\Dissent\dev"));
        assert_eq!(back, normal_dir(Path::new("C:/Users/Me/Dissent/dev/")));
        assert_eq!(back, normal_dir(Path::new("C:/Users/Me/Dissent/dev")));
        if cfg!(windows) {
            assert_eq!(back, normal_dir(Path::new(r"c:\users\me\dissent\DEV")));
        } else {
            assert_ne!(back, normal_dir(Path::new("/home/me/Dissent/dev")));
        }
    }

    /// Touches the real OS credential store, so it is ignored by default. It uses a test app name, never the app's own.
    /// Run with: `cargo test -p dissent-identity keychain -- --ignored`
    #[test]
    #[ignore]
    fn open_existing_never_creates_a_key() {
        let app = "dissent-identity-test";
        let profile = format!("absent-{:016x}", rand::random::<u64>());
        assert!(Vault::open_existing(app, &profile).unwrap().is_none());
        assert!(matches!(
            keyring::Entry::new(app, &profile).unwrap().get_secret(),
            Err(keyring::Error::NoEntry)
        ));
    }

    /// Touches the real OS credential store, so it is ignored by default. A legacy key is copied once, and the old
    /// entry is kept.
    /// Run with: `cargo test -p dissent-identity keychain -- --ignored`
    #[test]
    #[ignore]
    fn adopt_key_copies_a_legacy_key_once_and_keeps_the_old_entry() {
        let app = "dissent-identity-test";
        let from = format!("legacy-{:016x}", rand::random::<u64>());
        let to = format!("moved-{:016x}", rand::random::<u64>());
        let original = Vault::open(app, &from).unwrap();
        let sealed = original.seal(AAD, b"x");
        assert!(Vault::adopt_key(app, &from, &to).unwrap());
        assert!(!Vault::adopt_key(app, &from, &to).unwrap(), "a second copy changes nothing");
        let moved = Vault::open_existing(app, &to).unwrap().expect("copied");
        assert_eq!(moved.open_sealed(AAD, &sealed).unwrap(), b"x");
        assert!(Vault::open_existing(app, &from).unwrap().is_some(), "the legacy entry is kept");
        for profile in [&from, &to] {
            keyring::Entry::new(app, profile).unwrap().delete_credential().unwrap();
        }
    }

    /// Touches the real OS credential store, so it is ignored by default. The legacy key wins over a key made under
    /// the new name, since the legacy one sealed the data.
    /// Run with: `cargo test -p dissent-identity keychain -- --ignored`
    #[test]
    #[ignore]
    fn replace_key_overwrites_a_key_made_under_the_new_name() {
        let app = "dissent-identity-test";
        let from = format!("legacy-{:016x}", rand::random::<u64>());
        let to = format!("moved-{:016x}", rand::random::<u64>());
        let sealing = Vault::open(app, &from).unwrap();
        let sealed = sealing.seal(AAD, b"x");
        let stray = Vault::open(app, &to).unwrap();
        assert!(stray.open_sealed(AAD, &sealed).is_err());
        assert!(Vault::replace_key(app, &from, &to).unwrap());
        let replaced = Vault::open_existing(app, &to).unwrap().expect("replaced");
        assert_eq!(replaced.open_sealed(AAD, &sealed).unwrap(), b"x");
        for profile in [&from, &to] {
            keyring::Entry::new(app, profile).unwrap().delete_credential().unwrap();
        }
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
        keyring::Entry::new(app, &profile)
            .unwrap()
            .delete_credential()
            .unwrap();
    }
}
