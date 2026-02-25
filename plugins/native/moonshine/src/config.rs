// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

/// Configuration for the Moonshine STT plugin
#[derive(Serialize, Deserialize, Clone)]
pub struct MoonshineConfig {
    /// Path to the Moonshine model directory (containing encoder_model.ort,
    /// decoder_model_merged.ort, tokenizer.bin)
    #[serde(default = "default_model_dir")]
    pub model_dir: String,

    /// Model architecture to use
    #[serde(default = "default_model_arch")]
    pub model_arch: String,

    /// Number of threads for inference (0 = auto)
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,
}

fn default_model_dir() -> String {
    "models/moonshine-base-en".to_string()
}

fn default_model_arch() -> String {
    "base".to_string()
}

const fn default_num_threads() -> i32 {
    4
}

impl Default for MoonshineConfig {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            model_arch: default_model_arch(),
            num_threads: default_num_threads(),
        }
    }
}

impl MoonshineConfig {
    /// Convert the model_arch string to the corresponding FFI constant.
    pub fn arch_to_ffi(&self) -> Result<i32, String> {
        match self.model_arch.as_str() {
            "tiny" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_TINY),
            "base" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_BASE),
            "tiny_streaming" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_TINY_STREAMING),
            "base_streaming" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_BASE_STREAMING),
            "small_streaming" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_SMALL_STREAMING),
            "medium_streaming" => Ok(crate::ffi::MOONSHINE_MODEL_ARCH_MEDIUM_STREAMING),
            other => Err(format!(
                "Unknown model_arch '{other}'. Valid options: tiny, base, \
                 tiny_streaming, base_streaming, small_streaming, medium_streaming"
            )),
        }
    }

    /// Whether this config uses a streaming model architecture.
    pub fn is_streaming(&self) -> bool {
        matches!(
            self.model_arch.as_str(),
            "tiny_streaming" | "base_streaming" | "small_streaming" | "medium_streaming"
        )
    }
}
