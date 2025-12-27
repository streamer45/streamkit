// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KokoroTtsConfig {
    /// Path to model directory (contains model.onnx, voices.bin, etc.)
    pub model_dir: String,

    /// Speaker ID (0-102 for v1.1)
    #[serde(default = "default_speaker_id")]
    pub speaker_id: i32,

    /// Speech speed (0.5-2.0, default 1.0)
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// CPU threads for inference
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,

    /// Minimum characters before triggering TTS
    #[serde(default = "default_min_sentence_length")]
    pub min_sentence_length: usize,

    /// ONNX Runtime execution provider: "cpu", "cuda", "tensorrt"
    #[serde(default = "default_execution_provider")]
    pub execution_provider: String,

    /// Emit out-of-band telemetry events (tts.start/tts.done) to the session telemetry bus.
    #[serde(default)]
    pub emit_telemetry: bool,

    /// Maximum characters of text preview to include in telemetry events (0 = omit preview).
    #[serde(default = "default_telemetry_preview_chars")]
    pub telemetry_preview_chars: usize,
}

const fn default_speaker_id() -> i32 {
    50
}
const fn default_speed() -> f32 {
    1.0
}
const fn default_num_threads() -> i32 {
    4
}
const fn default_min_sentence_length() -> usize {
    10
}
fn default_execution_provider() -> String {
    // Check environment variable for default provider (useful for GPU containers)
    std::env::var("KOKORO_EXECUTION_PROVIDER").unwrap_or_else(|_| "cpu".to_string())
}

const fn default_telemetry_preview_chars() -> usize {
    80
}

impl Default for KokoroTtsConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/kokoro-multi-lang-v1_1".to_string(),
            speaker_id: 50,
            speed: 1.0,
            num_threads: 4,
            min_sentence_length: 10,
            execution_provider: "cpu".to_string(),
            emit_telemetry: false,
            telemetry_preview_chars: default_telemetry_preview_chars(),
        }
    }
}
