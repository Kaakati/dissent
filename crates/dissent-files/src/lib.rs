//! Encrypted file sharing over iroh-blobs. See docs/design/08-files.md.
//!
//! Each file is encrypted before it is stored, so peers and seeders hold only ciphertext. The
//! ciphertext is content-addressed by BLAKE3 and moves over iroh connections. The key and name
//! travel in a [`FileRef`], which the sender puts inside an MLS-sealed message.

mod crypt;
mod error;
mod file_ref;
mod name;
mod store;

pub use crypt::{chunk_count, cipher_len, decrypt, encrypt, FileKey, CHUNK_SIZE, MAX_PLAINTEXT};
pub use error::{FilesError, Result};
pub use file_ref::FileRef;
pub use iroh_blobs::BlobsProtocol;
pub use name::{sanitize_mime, sanitize_name, MAX_NAME_LEN};
pub use store::{Files, Input, Timeouts, DEFAULT_GC_INTERVAL, MAX_FILE_SIZE};

/// ALPN the blob protocol runs on: `/iroh-bytes/4`. Mount [`Files::protocol_handler`] under it.
pub const ALPN: &[u8] = iroh_blobs::ALPN;
