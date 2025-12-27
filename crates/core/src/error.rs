// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Structured error types for StreamKit.
//!
//! This module provides a hierarchy of error types for better error handling
//! and programmatic error inspection. All errors implement `Display` and can
//! be converted to/from `String` for backward compatibility.

use thiserror::Error;

/// Main error type for StreamKit operations.
///
/// This enum categorizes errors into distinct types to enable better error handling,
/// logging, and recovery strategies. Each variant includes a descriptive message.
#[derive(Debug, Error)]
pub enum StreamKitError {
    /// Configuration or parameter validation error.
    ///
    /// Examples:
    /// - Invalid node parameters (negative gain, invalid sample rate)
    /// - Missing required configuration fields
    /// - Invalid pipeline structure (circular dependencies)
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Runtime processing error during normal operation.
    ///
    /// Examples:
    /// - Audio buffer processing failure
    /// - Codec encoding/decoding error
    /// - Data format conversion failure
    #[error("Runtime error: {0}")]
    Runtime(String),

    /// Network-related error (sockets, HTTP, WebSocket, etc.).
    ///
    /// Examples:
    /// - Connection timeout
    /// - Socket closed unexpectedly
    /// - HTTP request failed
    #[error("Network error: {0}")]
    Network(String),

    /// Codec-specific error (encoding, decoding, format negotiation).
    ///
    /// Examples:
    /// - Opus encoder initialization failed
    /// - Invalid audio format for codec
    /// - Unsupported codec feature
    #[error("Codec error: {0}")]
    Codec(String),

    /// Plugin loading, initialization, or execution error.
    ///
    /// Examples:
    /// - Plugin file not found
    /// - ABI version mismatch
    /// - Plugin initialization failed
    /// - Plugin processing error
    #[error("Plugin error: {0}")]
    Plugin(String),

    /// I/O error (file operations, device access).
    ///
    /// Examples:
    /// - File not found
    /// - Permission denied
    /// - Disk full
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Resource exhaustion or limit exceeded.
    ///
    /// Examples:
    /// - Memory allocation failed
    /// - Too many open files
    /// - Queue capacity exceeded
    #[error("Resource exhaustion: {0}")]
    ResourceExhausted(String),
}

/// Convenience type alias for Results using `StreamKitError`.
pub type Result<T> = std::result::Result<T, StreamKitError>;

// Backward compatibility: Allow conversion from StreamKitError to String
impl From<StreamKitError> for String {
    fn from(err: StreamKitError) -> Self {
        err.to_string()
    }
}

// Backward compatibility: Allow conversion from String to StreamKitError
// This defaults to Runtime error for generic string errors
impl From<String> for StreamKitError {
    fn from(s: String) -> Self {
        Self::Runtime(s)
    }
}

// Backward compatibility: Allow conversion from &str to StreamKitError
impl From<&str> for StreamKitError {
    fn from(s: &str) -> Self {
        Self::Runtime(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = StreamKitError::Configuration("Invalid sample rate".to_string());
        assert_eq!(err.to_string(), "Configuration error: Invalid sample rate");

        let err = StreamKitError::Network("Connection timeout".to_string());
        assert_eq!(err.to_string(), "Network error: Connection timeout");
    }

    #[test]
    fn test_error_to_string_conversion() {
        let err = StreamKitError::Runtime("Processing failed".to_string());
        let s: String = err.into();
        assert_eq!(s, "Runtime error: Processing failed");
    }

    #[test]
    fn test_string_to_error_conversion() {
        let err: StreamKitError = "Something went wrong".into();
        assert_eq!(err.to_string(), "Runtime error: Something went wrong");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        let err: StreamKitError = io_err.into();
        assert!(err.to_string().contains("I/O error"));
        assert!(err.to_string().contains("File not found"));
    }
}
