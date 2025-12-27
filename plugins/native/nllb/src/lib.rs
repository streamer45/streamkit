// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A native plugin for real-time neural machine translation using NLLB-200
//!
//! This plugin provides high-performance CPU-based translation supporting 200 languages
//! using Meta's NLLB (No Language Left Behind) models via CTranslate2.
//!
//! # License Warning
//!
//! NLLB-200 models are licensed under CC-BY-NC-4.0 (Non-Commercial).
//! This plugin is suitable for research, education, and non-commercial use only.
//! For commercial deployments, consider Opus-MT models (Apache 2.0) instead.

#![allow(clippy::disallowed_macros)] // eprintln used for debugging
#![allow(clippy::uninlined_format_args)] // Legacy code style
#![allow(clippy::cognitive_complexity)] // Complex initialization logic
#![allow(clippy::field_reassign_with_default)] // CT2 config pattern

use ct2rs::tokenizers::auto::Tokenizer as AutoTokenizer;
use ct2rs::{Config, Device, Translator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use streamkit_plugin_sdk_native::prelude::*;

// Static initializer to test library loading at module load time
#[ctor::ctor]
fn init() {
    eprintln!("[NLLB Plugin] Module loaded - checking CTranslate2 availability");
    // This will run when the plugin is first loaded
    // If ct2rs can't find libctranslate2.so, it should fail here
}

/// Configuration for the NLLB translation plugin
#[derive(Serialize, Deserialize, Clone)]
struct TranslationConfig {
    /// Path to the CTranslate2 model directory
    #[serde(default = "default_model_path")]
    model_path: String,

    /// Source language code (NLLB format, e.g., "eng_Latn" for English)
    #[serde(default = "default_source_language")]
    source_language: String,

    /// Target language code (NLLB format, e.g., "spa_Latn" for Spanish)
    #[serde(default = "default_target_language")]
    target_language: String,

    /// Beam size for translation (1 = greedy, higher = better quality but slower)
    #[serde(default = "default_beam_size")]
    beam_size: usize,

    /// Number of threads to use (0 = auto)
    #[serde(default)]
    num_threads: usize,

    /// Device to use: "cpu", "cuda", or "auto"
    #[serde(default = "default_device")]
    device: String,

    /// GPU device ID (only used when device is "cuda")
    #[serde(default)]
    device_index: i32,
}

fn default_model_path() -> String {
    "models/nllb-200-distilled-600M-ct2-int8".to_string()
}

fn default_source_language() -> String {
    "eng_Latn".to_string()
}

fn default_target_language() -> String {
    "spa_Latn".to_string()
}

const fn default_beam_size() -> usize {
    1 // Greedy decoding for speed (use 4 for quality)
}

fn default_device() -> String {
    "cpu".to_string()
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            source_language: default_source_language(),
            target_language: default_target_language(),
            beam_size: default_beam_size(),
            num_threads: 0,
            device: default_device(),
            device_index: 0,
        }
    }
}

/// Wrapper for cached CTranslate2 translator
#[derive(Clone)]
struct CachedTranslator {
    translator: Arc<Translator<AutoTokenizer>>,
}

/// Global cache of translators
/// Key: (model_path, device, device_index)
type TranslatorCacheKey = (String, String, i32);

#[allow(clippy::type_complexity)]
static TRANSLATOR_CACHE: std::sync::LazyLock<Mutex<HashMap<TranslatorCacheKey, CachedTranslator>>> =
    std::sync::LazyLock::new(|| {
        eprintln!("[NLLB Plugin] Initializing NLLB translator cache");
        Mutex::new(HashMap::new())
    });

/// GPU availability status
/// 0 = not checked, 1 = available, 2 = not available
static GPU_AVAILABILITY: AtomicU8 = AtomicU8::new(0);

/// Check if GPU/CUDA is available by looking for CUDA libraries and devices.
/// This is done once and cached to avoid repeated checks.
fn is_gpu_available() -> bool {
    let status = GPU_AVAILABILITY.load(Ordering::Relaxed);

    // Already checked
    if status != 0 {
        return status == 1;
    }

    eprintln!("[NLLB Plugin] Checking GPU/CUDA availability (one-time check)");

    let available = check_cuda_available();

    GPU_AVAILABILITY.store(if available { 1 } else { 2 }, Ordering::Relaxed);
    eprintln!("[NLLB Plugin] GPU availability check complete: available={}", available);

    available
}

/// Perform actual CUDA availability detection.
fn check_cuda_available() -> bool {
    // Check 1: Look for NVIDIA GPU device files
    if Path::new("/dev/nvidia0").exists() {
        eprintln!("[NLLB Plugin] Found /dev/nvidia0 - NVIDIA GPU device present");
        return true;
    }

    // Check 2: Look for libcuda.so in common paths
    // This library is provided by the NVIDIA driver
    let cuda_lib_paths = [
        "/usr/lib/x86_64-linux-gnu/libcuda.so",
        "/usr/lib/x86_64-linux-gnu/libcuda.so.1",
        "/usr/local/cuda/lib64/libcudart.so",
        "/usr/lib/libcuda.so",
        "/usr/lib64/libcuda.so",
    ];

    for path in cuda_lib_paths {
        if Path::new(path).exists() {
            eprintln!("[NLLB Plugin] Found CUDA library: {} - GPU likely available", path);
            return true;
        }
    }

    // Check 3: Environment variable set by nvidia-container-toolkit
    if std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok() {
        eprintln!("[NLLB Plugin] NVIDIA_VISIBLE_DEVICES is set - running in GPU container");
        return true;
    }

    // Check 4: CUDA_VISIBLE_DEVICES (standard CUDA env var)
    if let Ok(val) = std::env::var("CUDA_VISIBLE_DEVICES") {
        // Empty or "-1" means no GPUs
        if !val.is_empty() && val != "-1" {
            eprintln!("[NLLB Plugin] CUDA_VISIBLE_DEVICES={} - GPU requested", val);
            return true;
        }
    }

    eprintln!("[NLLB Plugin] No GPU indicators found - assuming CPU-only environment");
    false
}

/// Parse device string to CTranslate2 enum with "auto" support.
/// "auto" will use CUDA if available, otherwise fallback to CPU.
fn parse_device(device: &str) -> Result<Device, String> {
    match device.to_lowercase().as_str() {
        "cpu" => Ok(Device::CPU),
        "cuda" => Ok(Device::CUDA),
        "auto" => {
            if is_gpu_available() {
                eprintln!("[NLLB Plugin] Auto-detected GPU, using CUDA");
                Ok(Device::CUDA)
            } else {
                eprintln!("[NLLB Plugin] Auto-detected CPU-only environment");
                Ok(Device::CPU)
            }
        },
        _ => Err(format!("Invalid device '{}'. Valid options: cpu, cuda, auto", device)),
    }
}

/// Normalize device string to actual device that will be used.
/// This is used for cache key to prevent mismatches.
fn normalize_device(device: &str) -> String {
    match device.to_lowercase().as_str() {
        "cpu" => "cpu".to_string(),
        "cuda" => {
            if is_gpu_available() {
                "cuda".to_string()
            } else {
                eprintln!(
                    "[NLLB Plugin] GPU device 'cuda' requested but not available, normalizing to 'cpu' for cache key"
                );
                "cpu".to_string()
            }
        },
        "auto" => {
            if is_gpu_available() {
                "cuda".to_string()
            } else {
                "cpu".to_string()
            }
        },
        _ => device.to_string(), // Let parse_device handle the error
    }
}

/// The NLLB translation plugin
pub struct NLLBPlugin {
    config: TranslationConfig,
    translator: Arc<Translator<AutoTokenizer>>,
    logger: Logger,
}

impl NativeProcessorNode for NLLBPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("nllb")
            .description(
                "Neural machine translation using Meta's NLLB (No Language Left Behind) model. \
                 Supports translation between 200+ languages. \
                 Accepts both text and transcription packets.",
            )
            .input("in", &[PacketType::Text, PacketType::Transcription])
            .output("out", PacketType::Text)
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_path": {
                        "type": "string",
                        "description": "Path to CTranslate2 model directory (see README for conversion instructions)",
                        "default": "models/nllb-200-distilled-600M-ct2-int8"
                    },
                    "source_language": {
                        "type": "string",
                        "description": "Source language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans')",
                        "default": "eng_Latn"
                    },
                    "target_language": {
                        "type": "string",
                        "description": "Target language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans')",
                        "default": "spa_Latn"
                    },
                    "beam_size": {
                        "type": "integer",
                        "description": "Beam search size (1 = greedy/fast, 4 = better quality/slower)",
                        "default": 1,
                        "minimum": 1,
                        "maximum": 10
                    },
                    "num_threads": {
                        "type": "integer",
                        "description": "Number of threads (0 = auto, recommended 4-8 for real-time)",
                        "default": 0,
                        "minimum": 0,
                        "maximum": 32
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
                    }
                }
            }))
            .category("ml")
            .category("translation")
            .category("text")
            .build()
    }

    fn new(params: Option<Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "NLLB plugin new() called with params: {:?}", params);

        let config: TranslationConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| {
                let error_msg = format!("Invalid config: {e}");
                plugin_error!(logger, "{}", error_msg);
                error_msg
            })?
        } else {
            TranslationConfig::default()
        };

        plugin_info!(
            logger,
            "Parsed config - model_path: {}, device: {}, source: {}, target: {}",
            config.model_path,
            config.device,
            config.source_language,
            config.target_language
        );

        // Parse device (handles "auto" to select GPU if available)
        let device = parse_device(&config.device)?;

        // Normalize device for cache key to prevent mismatches
        // e.g., if user requests "cuda" but GPU not available, use "cpu" in cache key
        let normalized_device = normalize_device(&config.device);

        // Cache key: only model-level parameters (not per-instance like source/target language)
        let cache_key = (config.model_path.clone(), normalized_device, config.device_index);

        // Get or create cached translator
        let translator = {
            let mut cache = TRANSLATOR_CACHE
                .lock()
                .map_err(|e| format!("Failed to lock translator cache: {e}"))?;

            if let Some(cached) = cache.get(&cache_key) {
                plugin_info!(
                    logger,
                    "✅ CACHE HIT: Reusing cached NLLB translator - model_path: {}, device: {}",
                    config.model_path,
                    config.device
                );
                cached.translator.clone()
            } else {
                plugin_info!(
                    logger,
                    "❌ CACHE MISS: Loading NLLB model (this may take a few seconds) - model_path: {}, device: {}, device_index: {}",
                    config.model_path,
                    config.device,
                    config.device_index
                );

                // Create translator configuration
                let mut ct2_config = Config::default();
                ct2_config.device = device;
                ct2_config.device_indices = vec![config.device_index];
                ct2_config.num_threads_per_replica = config.num_threads;

                // Load the model with tokenizer
                plugin_info!(logger, "Loading NLLB model from: {}", config.model_path);
                let translator = Translator::new(&config.model_path, &ct2_config).map_err(|e| {
                    let error_msg =
                        format!("Failed to load NLLB model from '{}': {:?}", config.model_path, e);
                    plugin_error!(logger, "{}", error_msg);
                    error_msg
                })?;

                let translator_arc = Arc::new(translator);

                // Cache for future use
                cache.insert(cache_key, CachedTranslator { translator: translator_arc.clone() });

                plugin_info!(logger, "✅ NLLB model loaded and cached");
                drop(cache); // Release lock early
                translator_arc
            }
        };

        // Validate language codes (basic check - NLLB uses BCP-47 variants)
        if config.source_language.is_empty() {
            return Err("source_language cannot be empty".to_string());
        }
        if config.target_language.is_empty() {
            return Err("target_language cannot be empty".to_string());
        }

        plugin_info!(
            logger,
            "NLLB translator initialized - source: {}, target: {}, beam_size: {}",
            config.source_language,
            config.target_language,
            config.beam_size
        );

        Ok(Self { config, translator, logger })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        // Extract text from either Text or Transcription packet.
        // Keep it borrowed to avoid extra copies on the hot path.
        let text = match &packet {
            Packet::Text(text) => text.as_ref(),
            Packet::Transcription(transcription) => transcription.text.as_str(),
            _ => return Err(format!("Expected Text or Transcription packet, got {:?}", packet)),
        };

        // Skip empty text
        if text.trim().is_empty() {
            return Ok(());
        }

        // Prepare input (single source text as a slice)
        let sources = vec![text];

        // Target prefix (nested vec for batch - single item with single prefix)
        let target_prefixes = vec![vec![self.config.target_language.as_str()]];

        // Create translation options
        let mut options = ct2rs::TranslationOptions::default();
        options.beam_size = self.config.beam_size;

        // Translate with target language prefix (no callback for now)
        let results = self
            .translator
            .translate_batch_with_target_prefix(
                &sources,
                &target_prefixes,
                &options,
                None, // No streaming callback
            )
            .map_err(|e| format!("Translation failed: {:?}", e))?;

        // Extract translated text (result is Vec<(String, Option<f32>)>)
        if let Some((translated, _score)) = results.first() {
            // Strip <unk> tokens from translation output (known NLLB artifact)
            let cleaned = if translated.contains("<unk>") {
                let cleaned = translated.replace("<unk>", "").trim().to_string();
                plugin_warn!(
                    self.logger,
                    "Stripped <unk> tokens from translation - original: '{}', raw: '{}', cleaned: '{}'",
                    text,
                    translated,
                    cleaned
                );
                cleaned
            } else {
                plugin_debug!(
                    self.logger,
                    "Translation completed - original: '{}', translated: '{}'",
                    text,
                    translated
                );
                translated.clone()
            };

            // Send translated text
            output.send("out", &Packet::Text(cleaned.into()))?;
        } else {
            plugin_warn!(self.logger, "Translation produced no results");
        }

        Ok(())
    }
}

// Export the plugin entry point
native_plugin_entry!(NLLBPlugin);
