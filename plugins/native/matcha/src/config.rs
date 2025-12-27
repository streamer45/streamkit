// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MatchaTtsConfig {
    /// Path to model directory (contains acoustic model, vocoder, tokens, etc.)
    pub model_dir: String,

    /// Speaker ID (typically 0 for single-speaker models like LJSpeech)
    #[serde(default = "default_speaker_id")]
    pub speaker_id: i32,

    /// Speech speed multiplier (0.5-2.0, default 1.0)
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// Noise scale for voice variation (default 0.667)
    #[serde(default = "default_noise_scale")]
    pub noise_scale: f32,

    /// Length scale for duration control (default 1.0, alternative to speed)
    #[serde(default = "default_length_scale")]
    pub length_scale: f32,

    /// CPU threads for inference
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,

    /// Minimum characters before triggering TTS
    #[serde(default = "default_min_sentence_length")]
    pub min_sentence_length: usize,

    /// ONNX Runtime execution provider: "cpu", "cuda", "tensorrt"
    #[serde(default = "default_execution_provider")]
    pub execution_provider: String,
}

const fn default_speaker_id() -> i32 {
    0
}
const fn default_speed() -> f32 {
    1.0
}
const fn default_noise_scale() -> f32 {
    0.667
}
const fn default_length_scale() -> f32 {
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
    std::env::var("MATCHA_EXECUTION_PROVIDER").unwrap_or_else(|_| "cpu".to_string())
}

impl Default for MatchaTtsConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/matcha-icefall-en_US-ljspeech".to_string(),
            speaker_id: 0,
            speed: 1.0,
            noise_scale: 0.667,
            length_scale: 1.0,
            num_threads: 4,
            min_sentence_length: 10,
            execution_provider: "cpu".to_string(),
        }
    }
}
