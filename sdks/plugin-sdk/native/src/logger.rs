// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Logging utilities for native plugins
//!
//! Provides a logger that sends log messages back to the host via callback.

use crate::types::{CLogCallback, CLogEnabledCallback, CLogLevel};
use std::ffi::CString;
use std::os::raw::c_void;
use std::sync::Arc;

/// Logger for sending log messages to the host
#[derive(Clone)]
pub struct Logger {
    callback: CLogCallback,
    enabled_callback: Option<CLogEnabledCallback>,
    user_data: *mut c_void,
    target: Arc<str>,
}

// SAFETY: The callback is a C function pointer which is thread-safe,
// and user_data is managed by the host which ensures thread-safety
unsafe impl Send for Logger {}
unsafe impl Sync for Logger {}

impl Logger {
    /// Create a new logger
    pub fn new(callback: CLogCallback, user_data: *mut c_void, target: &str) -> Self {
        Self { callback, enabled_callback: None, user_data, target: Arc::from(target) }
    }

    /// Set the enabled-check callback (called by the host for v9 plugins).
    pub fn set_enabled_callback(&mut self, callback: CLogEnabledCallback, user_data: *mut c_void) {
        self.enabled_callback = Some(callback);
        self.user_data = user_data;
    }

    /// Check whether the given log level is enabled.
    ///
    /// When no enabled callback is set (v8 host), always returns `true`.
    pub fn enabled(&self, level: CLogLevel) -> bool {
        match self.enabled_callback {
            Some(cb) => {
                let Ok(target_cstr) = CString::new(self.target.as_ref()) else {
                    return true;
                };
                cb(level, target_cstr.as_ptr(), self.user_data)
            },
            None => true,
        }
    }

    /// Log a message at the given level
    pub fn log(&self, level: CLogLevel, message: &str) {
        // Convert strings to C strings
        let Ok(target_cstr) = CString::new(self.target.as_ref()) else {
            return; // Silently ignore if target has null bytes
        };

        let Ok(message_cstr) = CString::new(message) else {
            return; // Silently ignore if message has null bytes
        };

        // Call the host's logging callback
        (self.callback)(level, target_cstr.as_ptr(), message_cstr.as_ptr(), self.user_data);
    }

    /// Log a trace message
    pub fn trace(&self, message: &str) {
        self.log(CLogLevel::Trace, message);
    }

    /// Log a debug message
    pub fn debug(&self, message: &str) {
        self.log(CLogLevel::Debug, message);
    }

    /// Log an info message
    pub fn info(&self, message: &str) {
        self.log(CLogLevel::Info, message);
    }

    /// Log a warning message
    pub fn warn(&self, message: &str) {
        self.log(CLogLevel::Warn, message);
    }

    /// Log an error message
    pub fn error(&self, message: &str) {
        self.log(CLogLevel::Error, message);
    }
}

/// Helper macro to format tracing-style field syntax into a simple string
#[doc(hidden)]
#[macro_export]
macro_rules! __format_fields {
    // Base case: just a format string
    ($fmt:literal) => {
        format!($fmt)
    };
    // Base case: format string with args
    ($fmt:literal, $($args:expr),+ $(,)?) => {
        format!($fmt, $($args),+)
    };
    // Field with % formatting (display) followed by more fields: field = %value, ...rest
    ($field:ident = %$value:expr, $($rest:tt)+) => {{
        let prefix = format!("{} = {}", stringify!($field), $value);
        let suffix = $crate::__format_fields!($($rest)+);
        if suffix.is_empty() {
            prefix
        } else {
            format!("{}, {}", prefix, suffix)
        }
    }};
    // Field with % formatting (display) - last field
    ($field:ident = %$value:expr) => {
        format!("{} = {}", stringify!($field), $value)
    };
    // Field with ? formatting (debug) followed by more fields: field = ?value, ...rest
    ($field:ident = ?$value:expr, $($rest:tt)+) => {{
        let prefix = format!("{} = {:?}", stringify!($field), $value);
        let suffix = $crate::__format_fields!($($rest)+);
        if suffix.is_empty() {
            prefix
        } else {
            format!("{}, {}", prefix, suffix)
        }
    }};
    // Field with ? formatting (debug) - last field
    ($field:ident = ?$value:expr) => {
        format!("{} = {:?}", stringify!($field), $value)
    };
    // Field without formatting followed by more fields: field = value, ...rest
    ($field:ident = $value:expr, $($rest:tt)+) => {{
        let prefix = format!("{} = {:?}", stringify!($field), $value);
        let suffix = $crate::__format_fields!($($rest)+);
        if suffix.is_empty() {
            prefix
        } else {
            format!("{}, {}", prefix, suffix)
        }
    }};
    // Field without formatting - last field
    ($field:ident = $value:expr) => {
        format!("{} = {:?}", stringify!($field), $value)
    };
}

/// Helper macros for logging with tracing-style field syntax support
#[macro_export]
macro_rules! plugin_log {
    ($logger:expr, $level:expr, $($arg:tt)*) => {
        $logger.log($level, &$crate::__format_fields!($($arg)*))
    };
}

#[macro_export]
macro_rules! plugin_trace {
    ($logger:expr, $($arg:tt)*) => {
        if $logger.enabled($crate::types::CLogLevel::Trace) {
            $logger.trace(&$crate::__format_fields!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! plugin_debug {
    ($logger:expr, $($arg:tt)*) => {
        if $logger.enabled($crate::types::CLogLevel::Debug) {
            $logger.debug(&$crate::__format_fields!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! plugin_info {
    ($logger:expr, $($arg:tt)*) => {
        if $logger.enabled($crate::types::CLogLevel::Info) {
            $logger.info(&$crate::__format_fields!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! plugin_warn {
    ($logger:expr, $($arg:tt)*) => {
        if $logger.enabled($crate::types::CLogLevel::Warn) {
            $logger.warn(&$crate::__format_fields!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! plugin_error {
    ($logger:expr, $($arg:tt)*) => {
        if $logger.enabled($crate::types::CLogLevel::Error) {
            $logger.error(&$crate::__format_fields!($($arg)*))
        }
    };
}
