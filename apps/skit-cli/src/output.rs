// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::Serialize;

/// Output format for CLI commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Wrapper for command output that can render as text or JSON.
pub struct CliOutput<T: Serialize> {
    format: OutputFormat,
    data: T,
    text_renderer: Box<dyn Fn(&T) -> String>,
}

impl<T: Serialize> CliOutput<T> {
    pub fn new(format: OutputFormat, data: T, text_fn: impl Fn(&T) -> String + 'static) -> Self {
        Self { format, data, text_renderer: Box::new(text_fn) }
    }

    pub fn print(&self) {
        match self.format {
            OutputFormat::Text => println!("{}", (self.text_renderer)(&self.data)),
            OutputFormat::Json => match serde_json::to_string_pretty(&self.data) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("Failed to serialize output as JSON: {e}");
                    std::process::exit(1);
                },
            },
        }
    }
}
