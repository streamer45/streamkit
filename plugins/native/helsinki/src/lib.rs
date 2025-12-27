// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Helsinki-NLP OPUS-MT Translation Plugin for StreamKit
//!
//! This plugin provides neural machine translation using Helsinki-NLP OPUS-MT models
//! via the Candle ML framework (pure Rust, no C dependencies).
//!
//! # Features
//!
//! - Bidirectional English-Spanish translation
//! - CPU and CUDA GPU support via Candle
//! - Model caching for efficient multi-instance usage
//! - Apache 2.0 licensed models (commercial-friendly)
//!
//! # License
//!
//! Plugin code: MPL-2.0
//! OPUS-MT models: Apache 2.0 (suitable for commercial use)

#![allow(clippy::disallowed_macros)]

mod config;
mod model;
mod translation;

use std::sync::{Arc, Mutex};

use serde_json::Value;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::{native_plugin_entry, plugin_debug, plugin_error, plugin_info};

use crate::config::HelsinkiConfig;
use crate::model::{get_or_load_translator, CachedTranslator};
use crate::translation::translate;

fn preview_for_log(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return "...".to_string();
    }
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),
        Some((byte_idx, _)) => format!("{}...", &text[..byte_idx]),
    }
}

fn canonicalize_model_dir(model_dir: &str) -> String {
    std::fs::canonicalize(model_dir)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| model_dir.to_string())
}

fn warmup_translate(
    logger: &Logger,
    translator: &Arc<Mutex<CachedTranslator>>,
    config: &HelsinkiConfig,
) {
    let mut warmup_config = config.clone();
    warmup_config.max_length = warmup_config.max_length.min(64);

    let start = std::time::Instant::now();
    match translate(translator, "Hello.", &warmup_config) {
        Ok(result) => {
            plugin_info!(
                logger,
                "Warmup translation completed in {}ms (output_len={})",
                start.elapsed().as_millis(),
                result.chars().count()
            );
        }
        Err(e) => {
            plugin_warn!(
                logger,
                "Warmup translation failed in {}ms: {}",
                start.elapsed().as_millis(),
                e
            );
        }
    }
}

/// The Helsinki-NLP OPUS-MT translation plugin.
pub struct HelsinkiPlugin {
    config: HelsinkiConfig,
    translator: Arc<Mutex<CachedTranslator>>,
    logger: Logger,
}

impl NativeProcessorNode for HelsinkiPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("helsinki")
            .description(
                "Neural machine translation using Helsinki-NLP OPUS-MT models. \
                 Supports bidirectional EN<->ES translation with Apache 2.0 licensed models. \
                 Powered by Candle (pure Rust ML framework).",
            )
            .input("in", &[PacketType::Text, PacketType::Transcription])
            .output("out", PacketType::Text)
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_dir": {
                        "type": "string",
                        "description": "Path to model directory containing safetensors and tokenizer files",
                        "default": "models/opus-mt-en-es"
                    },
                    "source_language": {
                        "type": "string",
                        "description": "Source language code: 'en' (English) or 'es' (Spanish)",
                        "default": "en",
                        "enum": ["en", "es"]
                    },
                    "target_language": {
                        "type": "string",
                        "description": "Target language code: 'en' (English) or 'es' (Spanish)",
                        "default": "es",
                        "enum": ["en", "es"]
                    },
                    "device": {
                        "type": "string",
                        "description": "Device to use: 'cpu', 'cuda', or 'auto'",
                        "default": "cpu",
                        "enum": ["cpu", "cuda", "auto"]
                    },
                    "device_index": {
                        "type": "integer",
                        "description": "GPU device index (only used when device is 'cuda')",
                        "default": 0,
                        "minimum": 0,
                        "maximum": 7
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum output sequence length",
                        "default": 512,
                        "minimum": 32,
                        "maximum": 2048
                    },
                    "warmup": {
                        "type": "boolean",
                        "description": "If true, run a small warmup translation during initialization to reduce first-request latency",
                        "default": false
                    }
                }
            }))
            .category("ml")
            .category("translation")
            .category("text")
            .build()
    }

    fn new(params: Option<Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(
            logger,
            "Helsinki plugin new() called with params: {:?}",
            params
        );

        let mut config: HelsinkiConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| {
                let error_msg = format!("Invalid config: {}", e);
                plugin_error!(logger, "{}", error_msg);
                error_msg
            })?
        } else {
            HelsinkiConfig::default()
        };

        // Validate configuration
        config.validate().map_err(|e| {
            plugin_error!(logger, "Configuration validation failed: {}", e);
            e
        })?;

        let canonical_model_dir = canonicalize_model_dir(&config.model_dir);
        if canonical_model_dir != config.model_dir {
            plugin_info!(
                logger,
                "Canonicalized model_dir: '{}' -> '{}'",
                config.model_dir,
                canonical_model_dir
            );
            config.model_dir = canonical_model_dir;
        }

        // Warn if model directory doesn't match language pair
        if let Err(e) = config.check_model_language_match() {
            plugin_error!(logger, "{}", e);
        }

        plugin_info!(
            logger,
            "Parsed config - model_dir: {}, device: {}, source: {}, target: {}",
            config.model_dir,
            config.device,
            config.source_language,
            config.target_language
        );

        // Get or load cached translator
        let translator = get_or_load_translator(&config, &logger)?;

        if config.warmup {
            warmup_translate(&logger, &translator, &config);
        }

        plugin_info!(logger, "Helsinki plugin initialized successfully");

        Ok(Self {
            config,
            translator,
            logger,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        // Extract text from packet
        let text: String = match &packet {
            Packet::Text(t) => t.as_ref().to_string(),
            Packet::Transcription(t) => t.text.clone(),
            _ => {
                return Err(format!(
                    "Expected Text or Transcription packet, got {:?}",
                    packet
                ))
            }
        };

        // Skip empty text
        if text.trim().is_empty() {
            plugin_debug!(self.logger, "Skipping empty text");
            return Ok(());
        }

        plugin_debug!(
            self.logger,
            "Translating {} chars: '{}'",
            text.chars().count(),
            preview_for_log(&text, 50)
        );

        // Translate
        let translated = translate(&self.translator, &text, &self.config).map_err(|e| {
            plugin_error!(self.logger, "Translation failed: {}", e);
            e
        })?;

        plugin_debug!(
            self.logger,
            "Translated to {} chars: '{}'",
            translated.chars().count(),
            preview_for_log(&translated, 50)
        );

        // Send translated text
        output.send("out", &Packet::Text(translated.into()))?;

        Ok(())
    }

    fn update_params(&mut self, params: Option<Value>) -> Result<(), String> {
        if let Some(p) = params {
            let mut new_config: HelsinkiConfig = serde_json::from_value(p)
                .map_err(|e| format!("Invalid config: {}", e))?;

            new_config.validate()?;

            let canonical_model_dir = canonicalize_model_dir(&new_config.model_dir);
            if canonical_model_dir != new_config.model_dir {
                plugin_info!(
                    self.logger,
                    "Canonicalized model_dir: '{}' -> '{}'",
                    new_config.model_dir,
                    canonical_model_dir
                );
                new_config.model_dir = canonical_model_dir;
            }

            // Check if model needs reloading (model-affecting params changed)
            let needs_reload = new_config.model_dir != self.config.model_dir
                || new_config.normalized_device() != self.config.normalized_device()
                || new_config.device_index != self.config.device_index;

            if needs_reload {
                plugin_info!(
                    self.logger,
                    "Model parameters changed, reloading translator"
                );
                self.translator = get_or_load_translator(&new_config, &self.logger)?;
            }

            // Update non-model params (these don't require reload)
            self.config = new_config;

            plugin_info!(self.logger, "Parameters updated successfully");
        }
        Ok(())
    }
}

native_plugin_entry!(HelsinkiPlugin);
