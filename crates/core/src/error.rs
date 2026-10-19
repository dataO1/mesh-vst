//! Errors of sample loading.

/// A sample file could not be read or converted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioFileError {
    #[error("I/O error: {0}")]
    IoError(String),
    #[error("invalid audio format: {0}")]
    InvalidFormat(String),
    #[error("corrupted audio file: {0}")]
    Corrupted(String),
}
