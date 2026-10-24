use thiserror::Error;

#[derive(Debug, Error)]
pub enum FilesError {
    #[error("Files can be up to 100 MB.")]
    TooLarge { size: u64, limit: u64 },
    #[error("file reference is malformed: {0}")]
    BadRef(String),
    #[error("ciphertext failed authentication (tampered, truncated, reordered or wrong key)")]
    Decrypt,
    #[error("encryption failed")]
    Crypto,
    #[error("no provider could supply the file: {0}")]
    Fetch(String),
    #[error("no providers were given")]
    NoProviders,
    #[error("blob store: {0}")]
    Store(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, FilesError>;
