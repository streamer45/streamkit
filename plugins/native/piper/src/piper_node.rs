// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Mutex;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{AudioFormat, SampleFormat};

use crate::config::PiperTtsConfig;
use crate::ffi;
use crate::sentence_splitter::SentenceSplitter;

/// Wrapper for TTS engine pointer that implements Send/Sync
/// SAFETY: We ensure thread-safe access through Mutex
struct TtsEnginePtr(*mut ffi::SherpaOnnxOfflineTts);
unsafe impl Send for TtsEnginePtr {}
unsafe impl Sync for TtsEnginePtr {}

/// Global cache of TTS engines
/// Key: (model_dir, num_threads)
static TTS_ENGINE_CACHE: std::sync::LazyLock<Mutex<HashMap<(String, i32), TtsEnginePtr>>> =
    std::sync::LazyLock::new(|| {
        tracing::info!("Initializing TTS engine cache with warm loading");
        let mut cache = HashMap::new();

        // Warm load: Pre-load default model to eliminate cold-start latency
        let default_config = PiperTtsConfig::default();
        let model_dir_path = PathBuf::from(&default_config.model_dir);
        let model_dir = if model_dir_path.is_absolute() {
            model_dir_path
        } else {
            std::env::current_dir().ok().map(|d| d.join(&model_dir_path)).unwrap_or(model_dir_path)
        };

        // Only pre-load if model exists (don't fail plugin load if model missing)
        if model_dir.exists() {
            tracing::info!(model_dir = %model_dir.display(), "Warm loading TTS model");
            match unsafe { create_tts_engine(&model_dir, &default_config) } {
                Ok(engine) => {
                    let cache_key =
                        (model_dir.to_string_lossy().to_string(), default_config.num_threads);
                    cache.insert(cache_key, TtsEnginePtr(engine));
                    tracing::info!("TTS model warm loaded successfully");
                },
                Err(e) => {
                    tracing::warn!("Failed to warm load TTS model: {}", e);
                },
            }
        } else {
            tracing::warn!(
                model_dir = %model_dir.display(),
                "Skipping warm load - model directory not found"
            );
        }

        Mutex::new(cache)
    });

pub struct PiperTtsNode {
    tts_engine: *mut ffi::SherpaOnnxOfflineTts,
    config: PiperTtsConfig,
    text_buffer: String,
    sentence_splitter: SentenceSplitter,
}

// SAFETY: We ensure thread-safety through proper synchronization
unsafe impl Send for PiperTtsNode {}
unsafe impl Sync for PiperTtsNode {}

impl NativeProcessorNode for PiperTtsNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("piper")
            .description(
                "Text-to-speech synthesis using Piper TTS models. \
                 Lightweight and efficient for real-time applications. \
                 Supports multiple voices and languages. Outputs 22.05kHz mono audio.",
            )
            .input("in", &[PacketType::Text])
            .output(
                "out",
                PacketType::RawAudio(AudioFormat {
                    sample_rate: 22050, // Piper models use 22.05kHz
                    channels: 1,
                    sample_format: SampleFormat::F32,
                }),
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_dir": {
                        "type": "string",
                        "description": "Path to Piper model directory",
                        "default": "./models/vits-piper-en_US-libritts_r-medium"
                    },
                    "speaker_id": {
                        "type": "integer",
                        "description": "Voice ID (model-dependent, 0-903 for libritts_r)",
                        "default": 0,
                        "minimum": 0,
                        "maximum": 903
                    },
                    "speed": {
                        "type": "number",
                        "description": "Speech speed multiplier",
                        "default": 1.0,
                        "minimum": 0.5,
                        "maximum": 2.0
                    },
                    "num_threads": {
                        "type": "integer",
                        "description": "CPU threads for inference",
                        "default": 4,
                        "minimum": 1,
                        "maximum": 16
                    },
                    "min_sentence_length": {
                        "type": "integer",
                        "description": "Minimum chars before TTS generation",
                        "default": 10,
                        "minimum": 1
                    },
                    "noise_scale": {
                        "type": "number",
                        "description": "Voice variation control (0.0-1.0)",
                        "default": 0.667,
                        "minimum": 0.0,
                        "maximum": 1.0
                    },
                    "noise_scale_w": {
                        "type": "number",
                        "description": "Prosody variation control (0.0-1.0)",
                        "default": 0.8,
                        "minimum": 0.0,
                        "maximum": 1.0
                    },
                    "length_scale": {
                        "type": "number",
                        "description": "Duration control (0.5-2.0)",
                        "default": 1.0,
                        "minimum": 0.5,
                        "maximum": 2.0
                    }
                },
                "required": ["model_dir"]
            }))
            .category("audio")
            .category("tts")
            .build()
    }

    fn new(params: Option<serde_json::Value>, _logger: Logger) -> Result<Self, String> {
        tracing::info!("PiperTtsNode::new() called");

        let config: PiperTtsConfig = if let Some(p) = params {
            tracing::info!("Parsing config from params");
            serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?
        } else {
            tracing::info!("Using default config");
            PiperTtsConfig::default()
        };

        tracing::info!(
            model_dir = %config.model_dir,
            speaker_id = config.speaker_id,
            "Config parsed successfully"
        );

        // Build paths
        tracing::info!("Building model_dir path");
        let model_dir = PathBuf::from(&config.model_dir);
        let model_dir = if model_dir.is_absolute() {
            tracing::info!("Model dir is absolute: {}", model_dir.display());
            model_dir
        } else {
            tracing::info!("Model dir is relative, making absolute");
            std::env::current_dir()
                .map_err(|e| format!("Failed to get current dir: {e}"))?
                .join(model_dir)
        };

        tracing::info!(model_dir = %model_dir.display(), "Final model directory");

        let model_dir_str = model_dir.to_string_lossy().to_string();
        // Cache key should only include model-level parameters
        let cache_key = (model_dir_str.clone(), config.num_threads);

        tracing::info!("Checking TTS engine cache");

        // Check cache first
        let tts_engine = {
            let mut cache =
                TTS_ENGINE_CACHE.lock().map_err(|e| format!("Failed to lock TTS cache: {e}"))?;

            if let Some(engine_ptr) = cache.get(&cache_key) {
                tracing::info!(
                    model_dir = %model_dir_str,
                    "Reusing cached TTS engine"
                );
                engine_ptr.0
            } else {
                tracing::info!(
                    model_dir = %model_dir_str,
                    "Creating new TTS engine"
                );

                let engine = unsafe { create_tts_engine(&model_dir, &config)? };
                cache.insert(cache_key, TtsEnginePtr(engine));
                engine
            }
        };

        let min_sentence_length = config.min_sentence_length;

        Ok(Self {
            tts_engine,
            config,
            text_buffer: String::new(),
            sentence_splitter: SentenceSplitter::new(min_sentence_length),
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        // Convert packet to text.
        // Keep it borrowed when possible to avoid unnecessary allocations.
        let text: std::borrow::Cow<'_, str> = match &packet {
            Packet::Text(text) => std::borrow::Cow::Borrowed(text.as_ref()),
            Packet::Binary { data, .. } => std::borrow::Cow::Owned(
                String::from_utf8(data.to_vec())
                    .map_err(|e| format!("Failed to decode binary data as UTF-8: {e}"))?,
            ),
            _ => return Err("Only accepts Text or Binary packets".to_string()),
        };

        tracing::debug!(text = %text, "Received text input");

        // Sanitize text before accumulating
        let mut sanitized = Self::sanitize_text(text.as_ref());
        tracing::debug!(sanitized = %sanitized, "Sanitized text");

        if sanitized.is_empty() {
            tracing::debug!("Text empty after sanitization, skipping");
            return Ok(());
        }

        // Add sentence-ending punctuation if missing
        if !sanitized.ends_with('.') && !sanitized.ends_with('!') && !sanitized.ends_with('?') {
            sanitized.push('.');
            tracing::debug!("Added sentence-ending punctuation");
        }

        // Accumulate text
        self.text_buffer.push_str(&sanitized);
        tracing::debug!(buffer = %self.text_buffer, buffer_len = self.text_buffer.len(), "Updated text buffer");

        // Extract and generate TTS for complete sentences
        while let Some(sentence) = self.sentence_splitter.extract_sentence(&mut self.text_buffer) {
            tracing::info!(sentence = %sentence, sentence_len = sentence.len(), "Generating TTS for sentence");
            self.generate_and_send(&sentence, output)?;
        }

        Ok(())
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        if let Some(p) = params {
            let new_config: PiperTtsConfig =
                serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?;

            // Update mutable parameters (those that don't require reloading the model)
            self.config.speaker_id = new_config.speaker_id;
            self.config.speed = new_config.speed;
            self.config.noise_scale = new_config.noise_scale;
            self.config.noise_scale_w = new_config.noise_scale_w;
            self.config.length_scale = new_config.length_scale;
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        // Flush any remaining buffered text
        if !self.text_buffer.is_empty() {
            tracing::info!("Text buffer not empty at cleanup, dropping");
        }
    }
}

impl PiperTtsNode {
    fn generate_and_send(&mut self, text: &str, output: &OutputSender) -> Result<(), String> {
        let text_cstr = CString::new(text).map_err(|e| format!("Invalid text: {e}"))?;

        // Use non-callback API for best performance
        unsafe {
            let audio_ptr = ffi::SherpaOnnxOfflineTtsGenerate(
                self.tts_engine,
                text_cstr.as_ptr(),
                self.config.speaker_id,
                self.config.speed,
            );

            if audio_ptr.is_null() {
                return Err("TTS generation failed".to_string());
            }

            // Read the generated audio
            let audio = &*audio_ptr;
            if audio.samples.is_null() || audio.n <= 0 {
                ffi::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio_ptr);
                return Err("TTS generated empty audio".to_string());
            }

            // Allow: Sample count from FFI is guaranteed positive (checked above)
            #[allow(clippy::cast_sign_loss)]
            let samples = std::slice::from_raw_parts(audio.samples, audio.n as usize);

            // Send all audio at once (simplest, lowest overhead)
            // Allow: Sample rate from FFI is guaranteed positive (audio format constraint)
            #[allow(clippy::cast_sign_loss)]
            let frame = AudioFrame::new(audio.sample_rate as u32, 1, samples.to_vec());
            output
                .send("out", &Packet::Audio(frame))
                .map_err(|e| format!("Failed to send audio: {e}"))?;

            // Destroy the audio object
            ffi::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio_ptr);
        }

        Ok(())
    }

    fn sanitize_text(text: &str) -> String {
        // Piper voices can be multilingual (e.g. Spanish), but the underlying TTS engine can
        // behave poorly with unexpected punctuation/symbols. Keep a conservative allowlist,
        // but include common Latin-1 accents (áéíóúüñ) and Spanish punctuation (¿¡).
        text.chars()
            .filter_map(|c| match c {
                'a'..='z'
                | 'A'..='Z'
                | '0'..='9'
                | ' '
                | '.'
                | ','
                | '!'
                | '?'
                | '-'
                | '\''
                | '"'
                | '\n'
                | ':'
                | ';'
                | 'à'..='ÿ'
                | 'À'..='Ÿ'
                | '¡'
                | '¿' => Some(c),
                c if c.is_whitespace() => Some(' '),
                _ => None,
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Create TTS engine using Sherpa-ONNX C API
unsafe fn create_tts_engine(
    model_dir: &Path,
    config: &PiperTtsConfig,
) -> Result<*mut ffi::SherpaOnnxOfflineTts, String> {
    tracing::info!(model_dir = %model_dir.display(), "Entering create_tts_engine");

    // Try to find the model file - support both naming conventions
    let model_path = if model_dir.join("model.onnx").exists() {
        // Sherpa-ONNX pre-converted naming
        model_dir.join("model.onnx")
    } else {
        // Original Piper naming (e.g., en_US-libritts_r-medium.onnx)
        // Look for any .onnx file
        let onnx_files: Vec<_> = std::fs::read_dir(model_dir)
            .map_err(|e| format!("Failed to read model directory: {e}"))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension()? == "onnx" && !path.to_string_lossy().contains(".onnx.") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        if onnx_files.is_empty() {
            return Err(format!("No .onnx model file found in {}", model_dir.display()));
        }
        if onnx_files.len() > 1 {
            tracing::warn!("Multiple .onnx files found, using first one: {:?}", onnx_files[0]);
        }
        onnx_files[0].clone()
    };

    let tokens_path = model_dir.join("tokens.txt");
    let data_dir = model_dir.join("espeak-ng-data");

    tracing::info!("Built all paths");

    // Verify files exist
    for (name, path) in [("model", &model_path), ("tokens", &tokens_path), ("data_dir", &data_dir)]
    {
        if !path.exists() {
            return Err(format!("{} file not found: {}", name, path.display()));
        }
        tracing::info!(file = %path.display(), "File exists: {}", name);
    }

    // Create C strings - keep them alive until after SherpaOnnxCreateOfflineTts call
    tracing::info!("Creating CStrings for paths");
    let model_cstr = path_to_cstring(&model_path)?;
    let tokens_cstr = path_to_cstring(&tokens_path)?;
    let data_dir_cstr = path_to_cstring(&data_dir)?;

    // IMPORTANT: Keep provider CStrings alive
    // Allow: Hard-coded string literals are known to be valid C strings (no null bytes)
    #[allow(clippy::unwrap_used)]
    let provider_cpu_cstr = CString::new("cpu").unwrap();
    #[allow(clippy::unwrap_used)]
    let empty_cstr = CString::new("").unwrap();

    tracing::info!("All CStrings created, building config struct");

    // Build config - match exact C API struct layout!
    let tts_config = ffi::SherpaOnnxOfflineTtsConfig {
        model: ffi::SherpaOnnxOfflineTtsModelConfig {
            // VITS config (what we actually use for Piper)
            vits: ffi::SherpaOnnxOfflineTtsVitsModelConfig {
                model: model_cstr.as_ptr(),
                lexicon: ptr::null(),
                tokens: tokens_cstr.as_ptr(),
                data_dir: data_dir_cstr.as_ptr(),
                noise_scale: config.noise_scale,
                noise_scale_w: config.noise_scale_w,
                length_scale: config.length_scale,
                dict_dir: ptr::null(),
            },
            // Common model config fields
            num_threads: config.num_threads,
            debug: 1,
            provider: provider_cpu_cstr.as_ptr(),
            // Matcha placeholder (unused)
            matcha: ffi::SherpaOnnxOfflineTtsMatchaModelConfig {
                acoustic_model: ptr::null(),
                vocoder: ptr::null(),
                lexicon: ptr::null(),
                tokens: ptr::null(),
                data_dir: ptr::null(),
                noise_scale: 0.0,
                length_scale: 1.0,
                dict_dir: ptr::null(),
            },
            // Kokoro placeholder (unused)
            kokoro: ffi::SherpaOnnxOfflineTtsKokoroModelConfig {
                model: ptr::null(),
                voices: ptr::null(),
                tokens: ptr::null(),
                data_dir: ptr::null(),
                length_scale: 1.0,
                dict_dir: ptr::null(),
                lexicon: ptr::null(),
                lang: ptr::null(),
            },
            // Kitten placeholder (unused)
            kitten: ffi::SherpaOnnxOfflineTtsKittenModelConfig {
                model: ptr::null(),
                voices: ptr::null(),
                tokens: ptr::null(),
                data_dir: ptr::null(),
                length_scale: 1.0,
            },
            // Zipvoice placeholder (unused)
            zipvoice: ffi::SherpaOnnxOfflineTtsZipvoiceModelConfig {
                tokens: ptr::null(),
                text_model: ptr::null(),
                flow_matching_model: ptr::null(),
                vocoder: ptr::null(),
                data_dir: ptr::null(),
                pinyin_dict: ptr::null(),
                feat_scale: 0.0,
                t_shift: 0.0,
                target_rms: 0.0,
                guidance_scale: 0.0,
            },
        },
        // Use empty string for rules
        rule_fsts: empty_cstr.as_ptr(),
        max_num_sentences: 1,
        rule_fars: empty_cstr.as_ptr(),
        silence_scale: 1.0,
    };

    tracing::info!(
        model = %model_path.display(),
        tokens = %tokens_path.display(),
        num_threads = config.num_threads,
        "About to call SherpaOnnxCreateOfflineTts"
    );

    let tts = ffi::SherpaOnnxCreateOfflineTts(&raw const tts_config);

    tracing::info!("SherpaOnnxCreateOfflineTts returned: ptr={:p}", tts);

    if tts.is_null() {
        return Err("Failed to create TTS engine".to_string());
    }

    tracing::info!("TTS engine created successfully");
    Ok(tts)
}

fn path_to_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| format!("Invalid path: {e}"))
}

impl Drop for PiperTtsNode {
    fn drop(&mut self) {
        // Note: We don't destroy the TTS engine here because it's cached
        // The cache will be cleared when the process exits
    }
}
