//! The per-call media secret.

use std::convert::Infallible;

use sframe::mls::MlsExporter;

/// The 32-byte secret both peers of a call share.
///
/// In Dissent this comes from the MLS exporter of the community group, so every group member
/// that is in the call derives the same key. The spike takes it as a plain value. The `Debug`
/// output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaKey([u8; 32]);

impl MediaKey {
    /// Length of the secret in bytes.
    pub const LEN: usize = 32;

    /// Wraps 32 secret bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for MediaKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaKey(<redacted>)")
    }
}

/// Treats the media key as an MLS exporter output, so `sframe` derives the per-sender keys
/// exactly as RFC 9605 section 5.2 describes. The real MLS group exporter plugs in here later.
impl MlsExporter for MediaKey {
    type BaseKey = [u8; 32];
    type Error = Infallible;

    fn export_secret(
        &self,
        _label: &str,
        _context: &[u8],
        _key_length: usize,
    ) -> Result<[u8; 32], Infallible> {
        Ok(self.0)
    }
}
