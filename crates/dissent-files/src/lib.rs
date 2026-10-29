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
mod tally;
mod thumbnail;

pub use crypt::{chunk_count, cipher_len, decrypt, encrypt, FileKey, CHUNK_SIZE, MAX_PLAINTEXT};
pub use error::{FilesError, Result};
pub use file_ref::FileRef;
pub use iroh_blobs::BlobsProtocol;
pub use name::{sanitize_mime, sanitize_name, MAX_NAME_LEN};
pub use tally::{pieces_of, SourceCount, PIECE_BYTES};
pub use thumbnail::{
    is_preview_image, is_thumbnail, make_preview_image, make_thumbnail, make_thumbnail_rgb, Thumbnail, PREVIEW_IMAGE_MAX_BYTES,
    PREVIEW_IMAGE_SIDE, THUMB_LONG_SIDE, THUMB_MAX_BYTES, THUMB_SOURCE_MAX,
};
pub use store::{
    Files, Input, MirrorPolicy, Timeouts, DEFAULT_GC_INTERVAL, DEFAULT_PARTIAL_TTL, DISK_MARGIN,
    MAX_BYTES_INPUT, MAX_FILE_SIZE,
};

/// What a blob server can be told about, and asked to allow: connections, requests and the bytes sent. A device
/// mounts [`Files::protocol_handler_with`] with an [`gate::EventSender`] to decide who may fetch what.
pub mod gate {
    pub use iroh_blobs::protocol::{GetManyRequest, GetRequest, ObserveRequest};
    pub use iroh_blobs::provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode, RequestUpdate,
        ThrottleMode,
    };
    pub use iroh_blobs::Hash;

    /// The transfer events of one request the gate allowed, under `RequestMode::InterceptLog`.
    pub type RequestUpdates = irpc::channel::mpsc::Receiver<RequestUpdate>;
}

/// ALPN the blob protocol runs on: `/iroh-bytes/4`. Mount [`Files::protocol_handler`] under it.
pub const ALPN: &[u8] = iroh_blobs::ALPN;
