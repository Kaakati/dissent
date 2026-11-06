//! Encrypted file sharing over iroh-blobs. See docs/design/08-files.md.
//!
//! Each file is encrypted before it is stored, so peers and seeders hold only ciphertext. The
//! ciphertext is content-addressed by BLAKE3 and moves over iroh connections. The key and name
//! travel in a [`FileRef`], which the sender puts inside an MLS-sealed message.

mod crypt;
mod error;
mod file_ref;
mod inplace;
mod name;
mod received;
mod store;
mod swarm;
mod tally;
mod thumbnail;
pub mod tracker;

pub use crypt::{chunk_count, cipher_len, decrypt, encrypt, FileKey, CHUNK_SIZE, MAX_PLAINTEXT};
pub use error::{FilesError, Result};

/// Bytes free on the volume that holds `dir`.
pub fn available_space(dir: &std::path::Path) -> std::io::Result<u64> {
    fs4::available_space(dir)
}
pub use file_ref::FileRef;
pub use inplace::{FilesProtocol, Linked, ServedHook};
pub use iroh_blobs::provider::events::EventSender;
pub use name::{sanitize_mime, sanitize_name, MAX_NAME_LEN};
pub use received::{danger_of_content, danger_of_name, danger_of_path, mark_received, Danger};
pub use swarm::{SwarmFetch, MAX_PARALLEL};
pub use tally::{pieces_of, SourceCount, PIECE_BYTES};
pub use thumbnail::{
    is_preview_image, is_thumbnail, make_preview_image, make_thumbnail, make_thumbnail_rgb, Thumbnail, PREVIEW_IMAGE_MAX_BYTES,
    PREVIEW_IMAGE_SIDE, THUMB_LONG_SIDE, THUMB_MAX_BYTES, THUMB_SOURCE_MAX,
};
pub use store::{
    Files, Input, MirrorPolicy, Timeouts, DEFAULT_GC_INTERVAL, DEFAULT_PARTIAL_TTL, DISK_MARGIN,
    MAX_BYTES_INPUT, MAX_FILE_SIZE, RELEASED_MAX_FILE_SIZE,
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

    /// A GET the gate is asked to allow, with the channel its answer goes on and its transfer events.
    pub type GetRequestReceivedMsg = irpc::WithChannels<
        iroh_blobs::provider::events::RequestReceived<GetRequest>,
        iroh_blobs::provider::events::ProviderProto,
    >;

    /// The transfer events of one request the gate allowed, under `RequestMode::InterceptLog`.
    pub type RequestUpdates = irpc::channel::mpsc::Receiver<RequestUpdate>;
}

/// ALPN the blob protocol runs on: `/iroh-bytes/4`. Mount [`Files::protocol_handler`] under it.
pub const ALPN: &[u8] = iroh_blobs::ALPN;

/// A size as people read it, in binary units: "512 KB", "10 GB", "1 TB". Whole numbers when exact, else one decimal.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value.fract() == 0.0 {
        format!("{} {}", value as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod size_text_tests {
    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(super::size_text(10 << 30), "10 GB");
        assert_eq!(super::size_text(1 << 40), "1 TB");
        assert_eq!(super::size_text(1536 << 20), "1.5 GB");
        assert_eq!(super::size_text(512), "512 bytes");
    }
}
