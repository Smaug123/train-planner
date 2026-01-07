//! Darwin client error types.

use std::fmt;

/// Errors from the Darwin HTTP client.
#[derive(Debug)]
pub enum DarwinError {
    /// HTTP request failed (network error, timeout, etc.)
    Http(reqwest::Error),

    /// JSON deserialization failed
    Json {
        message: String,
        body: Option<String>,
    },

    /// API returned an error status code
    ApiError { status: u16, message: String },

    /// Service details not found (expired or invalid ID)
    ServiceNotFound,

    /// Rate limited by the API
    RateLimited,

    /// Invalid API key or unauthorized
    Unauthorized,

    /// Feature not configured or not available
    NotConfigured(String),
}

impl fmt::Display for DarwinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DarwinError::Http(e) => write!(f, "HTTP error: {e}"),
            DarwinError::Json { message, body } => {
                write!(f, "JSON parse error: {message}")?;
                if let Some(body) = body {
                    write!(f, " (body: {body})")?;
                }
                Ok(())
            }
            DarwinError::ApiError { status, message } => {
                write!(f, "API error {status}: {message}")
            }
            DarwinError::ServiceNotFound => {
                write!(f, "service not found (expired or invalid ID)")
            }
            DarwinError::RateLimited => write!(f, "rate limited by Darwin API"),
            DarwinError::Unauthorized => write!(f, "unauthorized (invalid API key)"),
            DarwinError::NotConfigured(msg) => write!(f, "not configured: {msg}"),
        }
    }
}

impl std::error::Error for DarwinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DarwinError::Http(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for DarwinError {
    fn from(err: reqwest::Error) -> Self {
        DarwinError::Http(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = DarwinError::ServiceNotFound;
        assert_eq!(err.to_string(), "service not found (expired or invalid ID)");

        let err = DarwinError::ApiError {
            status: 500,
            message: "Internal Server Error".into(),
        };
        assert_eq!(err.to_string(), "API error 500: Internal Server Error");

        let err = DarwinError::Json {
            message: "expected string".into(),
            body: Some("{}".into()),
        };
        assert!(err.to_string().contains("JSON parse error"));
        assert!(err.to_string().contains("expected string"));
    }
}
