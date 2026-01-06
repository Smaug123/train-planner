//! Station API error types.

/// Errors that can occur when interacting with the Station API.
#[derive(Debug, thiserror::Error)]
pub enum StationError {
    /// HTTP request failed
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Authentication failed
    #[error("unauthorized: check DARWIN_USERNAME and DARWIN_PASSWORD")]
    Unauthorized,

    /// API returned an error status
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    /// Failed to parse response JSON
    #[error("JSON parse error: {message}")]
    Json { message: String },

    /// Cache operation failed
    #[error("cache error: {message}")]
    Cache { message: String },
}
