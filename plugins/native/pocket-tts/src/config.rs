// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PocketTtsConfig {
    /// Model variant (defaults to b6369a24).
    #[serde(default = "default_variant")]
    pub variant: String,

    /// Optional config YAML path for offline loading or custom variants.
    #[serde(default)]
    pub config_path: Option<String>,

    /// Optional local weights path for offline loading.
    #[serde(default)]
    pub weights_path: Option<String>,

    /// Optional local tokenizer path for offline loading.
    #[serde(default)]
    pub tokenizer_path: Option<String>,

    /// Optional directory containing predefined voice embeddings.
    #[serde(default)]
    pub voice_embeddings_dir: Option<String>,

    /// Voice specification (predefined name, hf:// URL, local file, or base64 audio).
    #[serde(default = "default_voice")]
    pub voice: String,

    /// Sampling temperature (higher = more variation).
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// LSD decode steps (higher = better quality, slower).
    #[serde(default = "default_lsd_decode_steps")]
    pub lsd_decode_steps: usize,

    /// EOS threshold (more negative = longer audio).
    #[serde(default = "default_eos_threshold")]
    pub eos_threshold: f32,

    /// Optional noise clamp for sampling.
    #[serde(default)]
    pub noise_clamp: Option<f32>,

    /// Minimum characters before triggering TTS.
    #[serde(default = "default_min_sentence_length")]
    pub min_sentence_length: usize,

    /// Enable simulated int8 quantization (requires plugin built with feature "quantized").
    #[serde(default)]
    pub quantized: bool,
}

const fn default_temperature() -> f32 {
    0.7
}

const fn default_lsd_decode_steps() -> usize {
    1
}

const fn default_eos_threshold() -> f32 {
    -4.0
}

const fn default_min_sentence_length() -> usize {
    10
}

fn default_voice() -> String {
    "alba".to_string()
}

fn default_variant() -> String {
    "b6369a24".to_string()
}

impl Default for PocketTtsConfig {
    fn default() -> Self {
        Self {
            variant: "b6369a24".to_string(),
            config_path: None,
            weights_path: None,
            tokenizer_path: None,
            voice_embeddings_dir: None,
            voice: default_voice(),
            temperature: default_temperature(),
            lsd_decode_steps: default_lsd_decode_steps(),
            eos_threshold: default_eos_threshold(),
            noise_clamp: None,
            min_sentence_length: default_min_sentence_length(),
            quantized: false,
        }
    }
}
