use std::fmt;

use serde::{Deserialize, Serialize};

use crate::crypt::{cipher_len, FileKey};
use crate::error::{FilesError, Result};
use crate::name::sanitize_name;
use crate::store::MAX_FILE_SIZE;

/// Everything a member needs to fetch and decrypt one shared file.
///
/// Send it inside an MLS-sealed message, so only group members learn `key`. The `hash` covers
/// the ciphertext, so seeders and relays can verify and serve the blob without the key.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// BLAKE3 hash of the ciphertext blob.
    pub hash: [u8; 32],
    /// Plaintext length in bytes.
    pub size: u64,
    /// Ciphertext length in bytes. Always `cipher_len(size)` for a file we made.
    pub cipher_size: u64,
    /// Sender's file name, untrusted. Use [`FileRef::safe_name`] before you show or save it.
    pub name: String,
    /// Sender's MIME type, untrusted.
    pub mime: String,
    pub key: [u8; 32],
    pub nonce_prefix: [u8; 8],
}

impl FileRef {
    /// Rejects references whose sizes are inconsistent or over `limit`, before any download.
    ///
    /// `limit` applies to the plaintext. The ciphertext may be slightly larger, by a fixed
    /// overhead per 64 KiB chunk.
    pub fn check(&self, limit: u64) -> Result<()> {
        let limit = limit.min(MAX_FILE_SIZE);
        if self.size > limit {
            return Err(FilesError::TooLarge {
                size: self.size,
                limit,
            });
        }
        // Ciphertext is the plaintext plus one tag per chunk. Nothing larger is acceptable.
        let max_cipher = cipher_len(limit);
        if self.cipher_size > max_cipher {
            return Err(FilesError::TooLarge {
                size: self.cipher_size,
                limit: max_cipher,
            });
        }
        if self.cipher_size != cipher_len(self.size) {
            return Err(FilesError::BadRef(
                "cipher_size does not match size".to_owned(),
            ));
        }
        Ok(())
    }

    /// The file name with path separators, control and bidi characters removed.
    pub fn safe_name(&self) -> String {
        sanitize_name(&self.name)
    }

    pub(crate) fn file_key(&self) -> FileKey {
        FileKey {
            key: self.key,
            nonce_prefix: self.nonce_prefix,
        }
    }
}

impl fmt::Debug for FileRef {
    // The key decrypts the file. Keep it out of logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileRef")
            .field(
                "hash",
                &blake3::Hash::from_bytes(self.hash).to_hex().as_str(),
            )
            .field("size", &self.size)
            .field("cipher_size", &self.cipher_size)
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(size: u64) -> FileRef {
        FileRef {
            hash: [7; 32],
            size,
            cipher_size: cipher_len(size),
            name: "../../evil".into(),
            mime: "text/plain".into(),
            key: [1; 32],
            nonce_prefix: [2; 8],
        }
    }

    #[test]
    fn check_rejects_size_over_limit() {
        assert!(sample(100).check(100).is_ok());
        assert!(matches!(
            sample(101).check(100),
            Err(FilesError::TooLarge { .. })
        ));
    }

    #[test]
    fn check_rejects_inconsistent_cipher_size() {
        let mut r = sample(100);
        r.cipher_size = 100;
        assert!(matches!(r.check(1000), Err(FilesError::BadRef(_))));
    }

    #[test]
    fn debug_hides_key() {
        let s = format!("{:?}", sample(10));
        assert!(!s.contains("[1, 1"), "{s}");
        assert!(s.contains("<redacted>"));
    }

    #[test]
    fn safe_name_neutralises_traversal() {
        assert_eq!(sample(1).safe_name(), "evil");
    }
}
