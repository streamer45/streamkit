// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PiperTtsConfig {
    /// Path to model directory (contains model.onnx, tokens.txt, etc.)
    pub model_dir: String,

    /// Speaker ID (for multi-speaker models)
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

    /// Noise scale (controls variation, 0.0-1.0)
    #[serde(default = "default_noise_scale")]
    pub noise_scale: f32,

    /// Noise scale W (controls prosody variation, 0.0-1.0)
    #[serde(default = "default_noise_scale_w")]
    pub noise_scale_w: f32,

    /// Length scale (controls speed, 0.5-2.0)
    #[serde(default = "default_length_scale")]
    pub length_scale: f32,
}

const fn default_speaker_id() -> i32 {
    0
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
const fn default_noise_scale() -> f32 {
    0.667
}
const fn default_noise_scale_w() -> f32 {
    0.8
}
const fn default_length_scale() -> f32 {
    1.0
}

impl Default for PiperTtsConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/vits-piper-en_US-libritts_r-medium".to_string(),
            speaker_id: 0,
            speed: 1.0,
            num_threads: 4,
            min_sentence_length: 10,
            noise_scale: 0.667,
            noise_scale_w: 0.8,
            length_scale: 1.0,
        }
    }
}
