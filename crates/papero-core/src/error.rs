use std::path::PathBuf;
use thiserror::Error;

/// Typed error enum for `papero-core` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("full-text index error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("query parser error: {0}")]
    TantivyQuery(#[from] tantivy::query::QueryParserError),

    #[error("filesystem watcher error: {0}")]
    Notify(#[from] notify::Error),

    #[error("invalid path '{0}': {1}")]
    InvalidPath(PathBuf, String),

    #[error("source root not found: {0}")]
    RootNotFound(String),

    #[error("document is encrypted or password-protected: {0}")]
    EncryptedFile(String),

    #[error("document is corrupt or unreadable: {0}")]
    CorruptFile(String),

    #[error("external tool missing ({tool}): {detail}")]
    MissingExternalTool { tool: String, detail: String },

    #[error("extraction error: {0}")]
    Extract(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("network/metadata client error: {0}")]
    Network(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("operation cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, CoreError>;
