// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

/// Configuration for the Parakeet TDT STT plugin
#[derive(Serialize, Deserialize, Clone)]
pub struct ParakeetConfig {
    /// Path to the Parakeet TDT model directory (contains encoder, decoder, joiner, tokens)
    #[serde(default = "default_model_dir")]
    pub model_dir: String,

    /// Number of threads for inference
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,

    /// Execution provider (cpu, cuda, tensorrt)
    #[serde(default = "default_execution_provider")]
    pub execution_provider: String,

    /// Enable VAD-based segmentation
    #[serde(default = "default_use_vad")]
    pub use_vad: bool,

    /// Path to Silero VAD model (if use_vad = true)
    #[serde(default = "default_vad_model_path")]
    pub vad_model_path: String,

    /// VAD speech probability threshold (0.0-1.0)
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,

    /// Minimum silence duration before triggering transcription (milliseconds)
    #[serde(default = "default_min_silence_duration_ms")]
    pub min_silence_duration_ms: u64,

    /// Maximum segment duration before forcing transcription (seconds)
    #[serde(default = "default_max_segment_duration_secs")]
    pub max_segment_duration_secs: f32,
}

fn default_model_dir() -> String {
    "models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8".to_string()
}

const fn default_num_threads() -> i32 {
    4
}

fn default_execution_provider() -> String {
    "cpu".to_string()
}

const fn default_use_vad() -> bool {
    true
}

fn default_vad_model_path() -> String {
    "models/silero_vad.onnx".to_string()
}

const fn default_vad_threshold() -> f32 {
    0.5
}

const fn default_min_silence_duration_ms() -> u64 {
    700
}

const fn default_max_segment_duration_secs() -> f32 {
    30.0
}

impl Default for ParakeetConfig {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            num_threads: default_num_threads(),
            execution_provider: default_execution_provider(),
            use_vad: default_use_vad(),
            vad_model_path: default_vad_model_path(),
            vad_threshold: default_vad_threshold(),
            min_silence_duration_ms: default_min_silence_duration_ms(),
            max_segment_duration_secs: default_max_segment_duration_secs(),
        }
    }
}
