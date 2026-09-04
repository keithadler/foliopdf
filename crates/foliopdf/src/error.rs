//! Error types shared across the crate.

use thiserror::Error;

/// Every fallible operation in `foliopdf` returns this error type.
///
/// Errors carry enough context to be shown to an end user; the CLI and the
/// WebAssembly bindings forward the [`Display`](std::fmt::Display) text as is.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The input is not a PDF file, or is damaged beyond what the recovery
    /// parser can rebuild.
    #[error("invalid PDF: {0}")]
    Malformed(String),

    /// A syntax error at a specific byte offset.
    #[error("syntax error at byte {offset}: {message}")]
    Syntax {
        /// Absolute offset into the input where parsing failed.
        offset: usize,
        /// Human-readable description.
        message: String,
    },

    /// An object was found but has the wrong type for the requested use.
    #[error("expected {expected}, found {found}")]
    Type {
        /// The type the caller asked for.
        expected: &'static str,
        /// The type actually present.
        found: &'static str,
    },

    /// A required dictionary key is missing.
    #[error("missing required key /{0}")]
    MissingKey(String),

    /// A stream filter is not implemented (for example JBIG2 decoding).
    #[error("unsupported filter /{0}")]
    UnsupportedFilter(String),

    /// The document is encrypted and the given password does not open it.
    #[error("the password does not open this document")]
    WrongPassword,

    /// The encryption method used by the document is not supported.
    #[error("unsupported encryption: {0}")]
    UnsupportedEncryption(String),

    /// Decompression failed, usually because a stream is truncated.
    #[error("decompression failed: {0}")]
    Decompress(String),

    /// A font file could not be parsed or does not contain required tables.
    #[error("font error: {0}")]
    Font(String),

    /// An image could not be decoded.
    #[error("image error: {0}")]
    Image(String),

    /// A page index was out of range.
    #[error("page index {index} out of range (document has {count} pages)")]
    PageOutOfRange {
        /// The requested zero-based page index.
        index: usize,
        /// The number of pages in the document.
        count: usize,
    },

    /// A page range expression such as `1-3,7` could not be parsed.
    #[error("invalid page range \"{0}\"")]
    PageRange(String),

    /// A batch preset or job description is invalid.
    #[error("invalid preset: {0}")]
    Preset(String),

    /// JSON (de)serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A limit meant to protect against hostile input was exceeded.
    #[error("resource limit exceeded: {0}")]
    Limit(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn malformed(msg: impl Into<String>) -> Self {
        Error::Malformed(msg.into())
    }
    pub(crate) fn syntax(offset: usize, msg: impl Into<String>) -> Self {
        Error::Syntax {
            offset,
            message: msg.into(),
        }
    }
}
