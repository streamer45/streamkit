// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration for the Helsinki-NLP OPUS-MT translation plugin.

use serde::{Deserialize, Serialize};

/// Configuration for the Helsinki OPUS-MT translation plugin.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HelsinkiConfig {
    /// Path to the model directory containing safetensors and tokenizer files.
    /// For EN->ES: "models/opus-mt-en-es"
    /// For ES->EN: "models/opus-mt-es-en"
    #[serde(default = "default_model_dir")]
    pub model_dir: String,

    /// Source language code: "en" or "es"
    #[serde(default = "default_source_language")]
    pub source_language: String,

    /// Target language code: "en" or "es"
    #[serde(default = "default_target_language")]
    pub target_language: String,

    /// Device to use: "cpu", "cuda", or "auto"
    #[serde(default = "default_device")]
    pub device: String,

    /// GPU device index (only used when device is "cuda")
    #[serde(default)]
    pub device_index: usize,

    /// Maximum output sequence length
    #[serde(default = "default_max_length")]
    pub max_length: usize,

    /// If true, run a small warmup translation during initialization to avoid
    /// first-request latency spikes (e.g. CUDA kernel initialization).
    #[serde(default)]
    pub warmup: bool,
}

fn default_model_dir() -> String {
    "models/opus-mt-en-es".to_string()
}

fn default_source_language() -> String {
    "en".to_string()
}

fn default_target_language() -> String {
    "es".to_string()
}

fn default_device() -> String {
    "cpu".to_string()
}

const fn default_max_length() -> usize {
    512
}

impl Default for HelsinkiConfig {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            source_language: default_source_language(),
            target_language: default_target_language(),
            device: default_device(),
            device_index: 0,
            max_length: default_max_length(),
            warmup: false,
        }
    }
}

impl HelsinkiConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate language codes
        let valid_languages = ["en", "es"];
        if !valid_languages.contains(&self.source_language.as_str()) {
            return Err(format!(
                "Invalid source_language '{}'. Must be one of: {:?}",
                self.source_language, valid_languages
            ));
        }
        if !valid_languages.contains(&self.target_language.as_str()) {
            return Err(format!(
                "Invalid target_language '{}'. Must be one of: {:?}",
                self.target_language, valid_languages
            ));
        }
        if self.source_language == self.target_language {
            return Err(format!(
                "source_language and target_language must be different, both are '{}'",
                self.source_language
            ));
        }

        // Validate device
        let valid_devices = ["cpu", "cuda", "auto"];
        if !valid_devices.contains(&self.device.to_lowercase().as_str()) {
            return Err(format!(
                "Invalid device '{}'. Must be one of: {:?}",
                self.device, valid_devices
            ));
        }

        // Validate max_length
        if self.max_length < 32 {
            return Err(format!(
                "max_length must be at least 32, got {}",
                self.max_length
            ));
        }
        if self.max_length > 2048 {
            return Err(format!(
                "max_length must be at most 2048, got {}",
                self.max_length
            ));
        }

        Ok(())
    }

    /// Get the normalized device string (lowercase).
    #[must_use]
    pub fn normalized_device(&self) -> String {
        self.device.to_lowercase()
    }

    /// Check if the model directory matches the expected language pair.
    pub fn check_model_language_match(&self) -> Result<(), String> {
        let model_dir_lower = self.model_dir.to_lowercase();

        // Expected pattern: opus-mt-{src}-{tgt}
        let expected_suffix = format!(
            "opus-mt-{}-{}",
            self.source_language, self.target_language
        );

        if !model_dir_lower.contains(&expected_suffix) {
            tracing::warn!(
                "Model directory '{}' may not match language pair {}->{}, expected path containing '{}'",
                self.model_dir,
                self.source_language,
                self.target_language,
                expected_suffix
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HelsinkiConfig::default();
        assert_eq!(config.source_language, "en");
        assert_eq!(config.target_language, "es");
        assert_eq!(config.device, "cpu");
        assert_eq!(config.max_length, 512);
    }

    #[test]
    fn test_validate_success() {
        let config = HelsinkiConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_same_language() {
        let config = HelsinkiConfig {
            source_language: "en".to_string(),
            target_language: "en".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_language() {
        let config = HelsinkiConfig {
            source_language: "fr".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}
