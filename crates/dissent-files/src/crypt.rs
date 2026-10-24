//! AES-256-GCM over a whole file, in 64 KiB chunks (the STREAM construction).
//!
//! Each file gets a fresh random 32-byte key and an 8-byte nonce prefix. Chunk `i` is sealed
//! with the 12-byte nonce `prefix || i (3 bytes, big endian) || last (1 byte)`. The last-chunk
//! flag stops truncation at a chunk boundary. The index stops reordering and swapping. The
//! file key is random for every file, so nonce reuse across files cannot happen.
//!
//! The nonce is 12 bytes, so the index has 3 bytes instead of 4. That caps a file at 2^24 chunks,
//! which is 1 TiB. Each chunk costs one GCM tag of 16 bytes, so a file of `n` plaintext bytes
//! has `cipher_len(n)` ciphertext bytes.

use std::fmt;
use std::io::{Read, Write};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

use crate::error::{FilesError, Result};

/// Plaintext bytes per chunk.
pub const CHUNK_SIZE: usize = 64 * 1024;
/// GCM tag bytes appended to every chunk.
pub const TAG_LEN: usize = 16;
const INDEX_BYTES: usize = 3;
/// Most chunks one file may have.
pub const MAX_CHUNKS: u64 = 1 << (8 * INDEX_BYTES);
/// Largest plaintext the scheme can address.
pub const MAX_PLAINTEXT: u64 = MAX_CHUNKS * CHUNK_SIZE as u64;
const AAD: &[u8] = b"dissent/files/chunk/v1";

/// Key material for one file. Carried inside an MLS-sealed `FileRef`, never stored in the blob.
#[derive(Clone, PartialEq, Eq)]
pub struct FileKey {
    pub key: [u8; 32],
    pub nonce_prefix: [u8; 8],
}

impl FileKey {
    /// A fresh random key and nonce prefix from the thread RNG.
    pub fn generate() -> Self {
        Self {
            key: rand::random(),
            nonce_prefix: rand::random(),
        }
    }
}

impl fmt::Debug for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileKey { .. }")
    }
}

/// Number of chunks a plaintext of `plain_len` bytes uses. An empty file still has one chunk.
pub fn chunk_count(plain_len: u64) -> u64 {
    if plain_len == 0 {
        1
    } else {
        plain_len.div_ceil(CHUNK_SIZE as u64)
    }
}

/// Ciphertext length for a plaintext of `plain_len` bytes.
pub fn cipher_len(plain_len: u64) -> u64 {
    plain_len + chunk_count(plain_len) * TAG_LEN as u64
}

/// The result of sealing one file. The caller stores the ciphertext and keeps `key`.
#[derive(Debug)]
pub(crate) struct Sealed {
    pub key: FileKey,
    pub plain_len: u64,
    pub cipher_len: u64,
}

/// Plaintext length for a ciphertext of `cipher_len` bytes, or `None` when no plaintext of that
/// length exists. This is the inverse of [`cipher_len`].
pub(crate) fn plain_len_for_cipher(ciphertext_len: u64) -> Option<u64> {
    let frame = (CHUNK_SIZE + TAG_LEN) as u64;
    let chunks = ciphertext_len.div_ceil(frame).max(1);
    let plain = ciphertext_len.checked_sub(chunks * TAG_LEN as u64)?;
    (cipher_len(plain) == ciphertext_len).then_some(plain)
}

/// Encrypts `input` into `output`, failing if the plaintext is over `limit`.
pub(crate) fn encrypt_stream<R, W>(input: &mut R, output: &mut W, limit: u64) -> Result<Sealed>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    let limit = limit.min(MAX_PLAINTEXT);
    let key = FileKey::generate();
    let cipher = cipher_for(&key)?;
    let mut cur = vec![0u8; CHUNK_SIZE];
    let mut next = vec![0u8; CHUNK_SIZE];
    let mut cur_len = read_full(input, &mut cur)?;
    let mut index = 0u64;
    let mut plain_len = 0u64;
    let mut out_len = 0u64;
    loop {
        plain_len += cur_len as u64;
        if plain_len > limit {
            return Err(FilesError::TooLarge {
                size: plain_len,
                limit,
            });
        }
        // Read one chunk ahead so we know whether the current one is the last.
        let next_len = read_full(input, &mut next)?;
        let last = next_len == 0;
        let sealed = seal(&cipher, &key, index, last, &cur[..cur_len])?;
        output.write_all(&sealed)?;
        out_len += sealed.len() as u64;
        if last {
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
        index += 1;
    }
    Ok(Sealed {
        key,
        plain_len,
        cipher_len: out_len,
    })
}

/// Decrypts `input` into `output` and returns the plaintext length. Any tampering, truncation,
/// reordering or wrong key fails, and `output` may already hold the chunks that verified.
pub(crate) fn decrypt_stream<R, W>(input: &mut R, output: &mut W, key: &FileKey) -> Result<u64>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    let cipher = cipher_for(key)?;
    let frame = CHUNK_SIZE + TAG_LEN;
    let mut cur = vec![0u8; frame];
    let mut next = vec![0u8; frame];
    let mut cur_len = read_full(input, &mut cur)?;
    if cur_len == 0 {
        // Zero bytes is not a valid stream. Every file has at least one sealed chunk.
        return Err(FilesError::Decrypt);
    }
    let mut index = 0u64;
    let mut plain_len = 0u64;
    loop {
        if index >= MAX_CHUNKS {
            return Err(FilesError::Decrypt);
        }
        let next_len = read_full(input, &mut next)?;
        let last = next_len == 0;
        let plain = open(&cipher, key, index, last, &cur[..cur_len])?;
        output.write_all(&plain)?;
        plain_len += plain.len() as u64;
        if last {
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
        index += 1;
    }
    Ok(plain_len)
}

/// Encrypts a whole byte slice in memory.
pub fn encrypt(plain: &[u8]) -> Result<(Vec<u8>, FileKey)> {
    let mut out = Vec::with_capacity(cipher_len(plain.len() as u64) as usize);
    let mut reader: &[u8] = plain;
    let sealed = encrypt_stream(&mut reader, &mut out, MAX_PLAINTEXT)?;
    Ok((out, sealed.key))
}

/// Decrypts a whole ciphertext in memory. Fails on any tampering, truncation or wrong key.
pub fn decrypt(cipher: &[u8], key: &FileKey) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut reader: &[u8] = cipher;
    decrypt_stream(&mut reader, &mut out, key)?;
    Ok(out)
}

fn cipher_for(key: &FileKey) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(&key.key).map_err(|_| FilesError::Crypto)
}

fn nonce(prefix: &[u8; 8], index: u64, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(prefix);
    n[8..8 + INDEX_BYTES].copy_from_slice(&index.to_be_bytes()[8 - INDEX_BYTES..]);
    n[11] = u8::from(last);
    n
}

fn seal(cipher: &Aes256Gcm, key: &FileKey, index: u64, last: bool, data: &[u8]) -> Result<Vec<u8>> {
    let n = nonce(&key.nonce_prefix, index, last);
    cipher
        .encrypt(
            Nonce::from_slice(&n),
            Payload {
                msg: data,
                aad: AAD,
            },
        )
        .map_err(|_| FilesError::Crypto)
}

fn open(cipher: &Aes256Gcm, key: &FileKey, index: u64, last: bool, data: &[u8]) -> Result<Vec<u8>> {
    let n = nonce(&key.nonce_prefix, index, last);
    cipher
        .decrypt(
            Nonce::from_slice(&n),
            Payload {
                msg: data,
                aad: AAD,
            },
        )
        .map_err(|_| FilesError::Decrypt)
}

/// Fills `buf` unless the input ends first. Returns how many bytes were read.
fn read_full<R: Read + ?Sized>(input: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    fn data(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8)
            .collect()
    }

    #[test]
    fn roundtrip_at_chunk_boundaries() {
        for n in [
            0,
            1,
            CHUNK_SIZE - 1,
            CHUNK_SIZE,
            CHUNK_SIZE + 1,
            3 * MIB + MIB / 2,
        ] {
            let plain = data(n);
            let (cipher, key) = encrypt(&plain).unwrap();
            assert_eq!(cipher.len() as u64, cipher_len(n as u64), "len for {n}");
            assert_eq!(decrypt(&cipher, &key).unwrap(), plain, "roundtrip for {n}");
        }
    }

    #[test]
    fn exact_chunk_has_no_trailing_empty_chunk() {
        let (cipher, _) = encrypt(&data(CHUNK_SIZE)).unwrap();
        assert_eq!(cipher.len(), CHUNK_SIZE + TAG_LEN);
        assert_eq!(chunk_count(CHUNK_SIZE as u64), 1);
        assert_eq!(chunk_count(CHUNK_SIZE as u64 + 1), 2);
        assert_eq!(chunk_count(0), 1);
    }

    #[test]
    fn ciphertext_hides_plaintext() {
        let plain = vec![0u8; 4096];
        let (cipher, _) = encrypt(&plain).unwrap();
        assert!(cipher.iter().filter(|&&b| b == 0).count() < 64);
    }

    #[test]
    fn tampered_byte_fails() {
        let (mut cipher, key) = encrypt(&data(3 * CHUNK_SIZE)).unwrap();
        let mid = cipher.len() / 2;
        cipher[mid] ^= 1;
        assert!(matches!(decrypt(&cipher, &key), Err(FilesError::Decrypt)));
    }

    #[test]
    fn swapped_chunks_fail() {
        let frame = CHUNK_SIZE + TAG_LEN;
        let (cipher, key) = encrypt(&data(3 * CHUNK_SIZE)).unwrap();
        let mut swapped = Vec::with_capacity(cipher.len());
        swapped.extend_from_slice(&cipher[frame..2 * frame]);
        swapped.extend_from_slice(&cipher[..frame]);
        swapped.extend_from_slice(&cipher[2 * frame..]);
        assert!(matches!(decrypt(&swapped, &key), Err(FilesError::Decrypt)));
    }

    #[test]
    fn truncation_fails() {
        let frame = CHUNK_SIZE + TAG_LEN;
        let plain = data(3 * CHUNK_SIZE + 100);
        let (cipher, key) = encrypt(&plain).unwrap();
        // Drop the final chunk whole: the previous chunk was sealed as not-last.
        let whole = &cipher[..3 * frame];
        assert!(matches!(decrypt(whole, &key), Err(FilesError::Decrypt)));
        // Cut the final chunk mid-way.
        let partial = &cipher[..cipher.len() - 7];
        assert!(matches!(decrypt(partial, &key), Err(FilesError::Decrypt)));
        // Cut mid-chunk at a chunk boundary inside the stream.
        let inner = &cipher[..frame + 5];
        assert!(matches!(decrypt(inner, &key), Err(FilesError::Decrypt)));
        // Empty input.
        assert!(matches!(decrypt(&[], &key), Err(FilesError::Decrypt)));
    }

    #[test]
    fn appended_chunk_fails() {
        let frame = CHUNK_SIZE + TAG_LEN;
        let (cipher, key) = encrypt(&data(2 * CHUNK_SIZE)).unwrap();
        let (extra, _) = encrypt(&data(10)).unwrap();
        let mut longer = cipher.clone();
        longer.extend_from_slice(&extra);
        // The original last chunk now looks non-last, so it fails.
        assert!(matches!(decrypt(&longer, &key), Err(FilesError::Decrypt)));
        assert_eq!(cipher.len(), 2 * frame);
    }

    #[test]
    fn wrong_key_fails() {
        let (cipher, key) = encrypt(b"secret").unwrap();
        let mut bad_key = key.clone();
        bad_key.key[0] ^= 1;
        assert!(matches!(
            decrypt(&cipher, &bad_key),
            Err(FilesError::Decrypt)
        ));
        let mut bad_prefix = key.clone();
        bad_prefix.nonce_prefix[0] ^= 1;
        assert!(matches!(
            decrypt(&cipher, &bad_prefix),
            Err(FilesError::Decrypt)
        ));
        assert_eq!(decrypt(&cipher, &key).unwrap(), b"secret");
    }

    #[test]
    fn streaming_matches_bytes_api() {
        let plain = data(MIB + 17);
        let mut cipher = Vec::new();
        let mut reader: &[u8] = &plain;
        let sealed = encrypt_stream(&mut reader, &mut cipher, MAX_PLAINTEXT).unwrap();
        assert_eq!(sealed.plain_len, plain.len() as u64);
        assert_eq!(sealed.cipher_len, cipher.len() as u64);
        let mut out = Vec::new();
        let mut cr: &[u8] = &cipher;
        let n = decrypt_stream(&mut cr, &mut out, &sealed.key).unwrap();
        assert_eq!(n, plain.len() as u64);
        assert_eq!(out, plain);
    }

    #[test]
    fn plaintext_over_limit_is_refused() {
        let plain = data(2 * CHUNK_SIZE);
        let mut out = Vec::new();
        let mut reader: &[u8] = &plain;
        let err = encrypt_stream(&mut reader, &mut out, CHUNK_SIZE as u64).unwrap_err();
        assert!(matches!(err, FilesError::TooLarge { .. }));
    }
}
