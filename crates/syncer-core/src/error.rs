use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("device name must contain 1..=64 non-control characters")]
    InvalidDeviceName,
    #[error("relative path is empty")]
    EmptyRelativePath,
    #[error("relative path must not be absolute")]
    AbsoluteRelativePath,
    #[error("relative path must not contain traversal components")]
    TraversalRelativePath,
    #[error("relative path must not target the .syncer metadata directory")]
    SyncerMetadataPath,
    #[error("folder size limit must be greater than zero")]
    InvalidFolderLimit,
    #[error("warning threshold must be between 1 and 100")]
    InvalidWarningThreshold,
    #[error("public key must contain exactly 32 bytes")]
    InvalidPublicKeyLength,
    #[error("pairing ticket must not be expired at creation time")]
    ExpiredPairingTicket,
    #[error("folder configuration schema version {found} is not supported")]
    UnsupportedFolderSchema { found: u16 },
    #[error("storage root must be a native filesystem path for this operation")]
    NativeStorageRootRequired,
    #[error("failed filesystem operation at {path}: {message}")]
    Filesystem { path: String, message: String },
    #[error("failed to serialize or deserialize JSON: {0}")]
    Json(String),
    #[error("state database error: {0}")]
    StateDatabase(String),
    #[error("file size exceeded supported range")]
    FileSizeOverflow,
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

impl From<sqlx::Error> for CoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::StateDatabase(error.to_string())
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Filesystem {
            path: String::new(),
            message: error.to_string(),
        }
    }
}
