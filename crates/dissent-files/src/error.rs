use thiserror::Error;

#[derive(Debug, Error)]
pub enum FilesError {
    #[error("This file is larger than the limit of {}.", crate::size_text(*limit))]
    TooLarge { size: u64, limit: u64 },
    #[error("Input::Bytes holds up to 64 MiB. Use Input::Path for larger files.")]
    BytesTooLarge { size: u64, max: u64 },
    #[error("not enough disk space: need {needed} bytes, {available} available")]
    DiskFull { needed: u64, available: u64 },
    #[error("seeder policy refused the file: {0}")]
    PolicyRefused(String),
    #[error("file reference is malformed: {0}")]
    BadRef(String),
    #[error("ciphertext failed authentication (tampered, truncated, reordered or wrong key)")]
    Decrypt,
    #[error("encryption failed")]
    Crypto,
    #[error("the file changed while it was being added")]
    Changed,
    #[error("no provider could supply the file: {0}")]
    Fetch(String),
    /// Every provider that was reached refused to serve this device: it is not a member of a group the blob belongs to,
    /// or the provider did not know this device yet.
    #[error("the providers refused to share the file: {0}")]
    Refused(String),
    #[error("no providers were given")]
    NoProviders,
    #[error("blob store: {0}")]
    Store(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, FilesError>;
