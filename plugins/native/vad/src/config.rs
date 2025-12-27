// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration structures for the VAD plugin

use serde::{Deserialize, Serialize};

/// Output mode for the VAD plugin
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VadOutputMode {
    /// Emit structured VAD events on speech state changes (start/stop)
    #[default]
    Events,
    /// Pass through only audio segments containing speech
    FilteredAudio,
}

/// Configuration for the VAD plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    /// Path to the ten-vad ONNX model
    #[serde(default = "default_model_path")]
    pub model_path: String,

    /// Output mode: events or filtered audio
    #[serde(default)]
    pub output_mode: VadOutputMode,

    /// VAD threshold (0.0 - 1.0)
    /// Higher values = more conservative (only very clear speech detected)
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    /// Minimum silence duration in seconds to trigger speech end
    #[serde(default = "default_min_silence_duration")]
    pub min_silence_duration_s: f32,

    /// Minimum speech duration in seconds to be considered valid speech
    #[serde(default = "default_min_speech_duration")]
    pub min_speech_duration_s: f32,

    /// Window size for VAD processing (in samples, typically 512 for 16kHz)
    #[serde(default = "default_window_size")]
    pub window_size: i32,

    /// Maximum speech duration in seconds (speech longer than this will be split)
    #[serde(default = "default_max_speech_duration")]
    pub max_speech_duration_s: f32,

    /// Number of threads for ONNX runtime
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,

    /// ONNX execution provider (e.g., "cpu", "cuda")
    #[serde(default = "default_provider")]
    pub provider: String,

    /// Enable debug logging from sherpa-onnx
    #[serde(default)]
    pub debug: bool,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            output_mode: VadOutputMode::default(),
            threshold: default_threshold(),
            min_silence_duration_s: default_min_silence_duration(),
            min_speech_duration_s: default_min_speech_duration(),
            window_size: default_window_size(),
            max_speech_duration_s: default_max_speech_duration(),
            num_threads: default_num_threads(),
            provider: default_provider(),
            debug: false,
        }
    }
}

fn default_model_path() -> String {
    "models/ten-vad.onnx".to_string()
}

fn default_threshold() -> f32 {
    0.5
}

fn default_min_silence_duration() -> f32 {
    0.5 // 500ms
}

fn default_min_speech_duration() -> f32 {
    0.25 // 250ms
}

fn default_window_size() -> i32 {
    512 // 32ms at 16kHz
}

fn default_max_speech_duration() -> f32 {
    30.0 // 30 seconds
}

fn default_num_threads() -> i32 {
    1
}

fn default_provider() -> String {
    "cpu".to_string()
}

/// Generate cache key for VAD detector (only model-affecting parameters)
pub fn vad_cache_key(config: &VadConfig) -> String {
    format!("{}|{}|{}", config.model_path, config.num_threads, config.provider)
}
