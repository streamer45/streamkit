// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SupertonicConfig {
    /// Path to ONNX model directory (contains duration_predictor.onnx, etc.)
    pub model_dir: String,

    /// Language code: "en", "ko", "es", "pt", "fr"
    #[serde(default = "default_lang")]
    pub lang: String,

    /// Voice style name (e.g. "M1", "F1") or path to .json file
    #[serde(default = "default_voice_style")]
    pub voice_style: String,

    /// Directory containing named voice style .json files
    #[serde(default)]
    pub voice_styles_dir: Option<String>,

    /// Denoising steps (1-20)
    #[serde(default = "default_total_step")]
    pub total_step: usize,

    /// Speed multiplier (0.5-2.0)
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// Silence between chunks in seconds
    #[serde(default = "default_silence_duration")]
    pub silence_duration: f32,

    /// Minimum characters before triggering TTS
    #[serde(default = "default_min_sentence_length")]
    pub min_sentence_length: usize,

    /// Emit out-of-band telemetry events (tts.start/tts.done)
    #[serde(default)]
    pub emit_telemetry: bool,

    /// Maximum characters of text preview to include in telemetry events (0 = omit preview)
    #[serde(default = "default_telemetry_preview_chars")]
    pub telemetry_preview_chars: usize,
}

fn default_lang() -> String {
    "en".to_string()
}

fn default_voice_style() -> String {
    "M1".to_string()
}

const fn default_total_step() -> usize {
    5
}

const fn default_speed() -> f32 {
    1.05
}

const fn default_silence_duration() -> f32 {
    0.3
}

const fn default_min_sentence_length() -> usize {
    10
}

const fn default_telemetry_preview_chars() -> usize {
    80
}

impl Default for SupertonicConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/supertonic-v2-onnx".to_string(),
            lang: default_lang(),
            voice_style: default_voice_style(),
            voice_styles_dir: None,
            total_step: default_total_step(),
            speed: default_speed(),
            silence_duration: default_silence_duration(),
            min_sentence_length: default_min_sentence_length(),
            emit_telemetry: false,
            telemetry_preview_chars: default_telemetry_preview_chars(),
        }
    }
}
