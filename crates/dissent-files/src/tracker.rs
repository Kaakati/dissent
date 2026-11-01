//! What the server file tracker needs from the file store: fingerprints and content keys derived from the server's
//! files key. See `docs/design/14-file-tracker.md`, "Encryption, fingerprints and dedupe".
//!
//! A file's key is derived from the files key of a generation and the file's fingerprint, so the same bytes under the
//! same generation always give the same ciphertext and blob hash: two members who list the same file make one swarm.
//! A key is bound to one fingerprint, so two different plaintexts never share a key and nonces.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::crypt::FileKey;
use crate::error::Result;

const KEY_DOMAIN: &str = "dissent/tracker-content-key/v1";
const NONCE_DOMAIN: &str = "dissent/tracker-content-nonce/v1";
/// How much one read of the fingerprint pass takes.
const READ_BYTES: usize = 1024 * 1024;

/// The key a file with `fingerprint` is encrypted under, in the generation whose files key is `files_key`.
pub fn content_key(files_key: &[u8; 32], fingerprint: &[u8; 32]) -> FileKey {
    let material = [files_key.as_slice(), fingerprint.as_slice()].concat();
    let key = blake3::derive_key(KEY_DOMAIN, &material);
    let nonce = blake3::derive_key(NONCE_DOMAIN, &material);
    let mut nonce_prefix = [0u8; 8];
    nonce_prefix.copy_from_slice(&nonce[..8]);
    FileKey { key, nonce_prefix }
}

/// The BLAKE3 hash of `reader`'s bytes and their length. `progress` gets the bytes hashed so far after each read, and
/// stops the pass when it returns false.
pub fn fingerprint_reader(reader: &mut impl Read, mut progress: impl FnMut(u64) -> bool) -> io::Result<([u8; 32], u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BYTES];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        total += n as u64;
        if !progress(total) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "fingerprinting was cancelled"));
        }
    }
    Ok((*hasher.finalize().as_bytes(), total))
}

/// The fingerprint and length of the file at `path`. Blocking: run it off the async runtime.
pub fn fingerprint_file(path: &Path, progress: impl FnMut(u64) -> bool) -> Result<([u8; 32], u64)> {
    let mut file = File::open(path)?;
    Ok(fingerprint_reader(&mut file, progress)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_file_and_generation_give_the_same_key() {
        let fp = *blake3::hash(b"release").as_bytes();
        assert!(content_key(&[1; 32], &fp) == content_key(&[1; 32], &fp));
        assert!(content_key(&[1; 32], &fp) != content_key(&[2; 32], &fp), "a new generation, a new key");
        let other = *blake3::hash(b"other").as_bytes();
        let (a, b) = (content_key(&[1; 32], &fp), content_key(&[1; 32], &other));
        assert!(a.key != b.key && a.nonce_prefix != b.nonce_prefix, "another file, another key and nonces");
    }

    #[test]
    fn fingerprints_match_blake3_and_can_stop() {
        let data = vec![7u8; 3 * READ_BYTES + 5];
        let (fp, len) = fingerprint_reader(&mut data.as_slice(), |_| true).unwrap();
        assert_eq!((fp, len), (*blake3::hash(&data).as_bytes(), data.len() as u64));
        let mut seen = Vec::new();
        let stopped = fingerprint_reader(&mut data.as_slice(), |n| {
            seen.push(n);
            n < READ_BYTES as u64 * 2
        });
        assert!(stopped.is_err());
        assert_eq!(seen.len(), 2);
    }
}
