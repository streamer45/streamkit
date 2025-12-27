// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{AudioFormat, SampleFormat};

use crate::config::KokoroTtsConfig;
use crate::ffi;
use crate::sentence_splitter::SentenceSplitter;

/// GPU availability status
/// 0 = not checked, 1 = available, 2 = not available
static GPU_AVAILABILITY: AtomicU8 = AtomicU8::new(0);

/// Check if GPU/CUDA is available by looking for CUDA libraries and devices.
/// This is done once and cached to avoid repeated checks.
fn is_gpu_available(logger: &Logger) -> bool {
    let status = GPU_AVAILABILITY.load(Ordering::Relaxed);

    // Already checked
    if status != 0 {
        return status == 1;
    }

    plugin_info!(logger, "Checking GPU/CUDA availability (one-time check)");

    let available = check_cuda_available(logger);

    GPU_AVAILABILITY.store(if available { 1 } else { 2 }, Ordering::Relaxed);
    plugin_info!(logger, "GPU availability check complete: available={}", available);

    available
}

/// Perform actual CUDA availability detection.
fn check_cuda_available(logger: &Logger) -> bool {
    // Check 1: Look for NVIDIA GPU device files
    if Path::new("/dev/nvidia0").exists() {
        plugin_info!(logger, "Found /dev/nvidia0 - NVIDIA GPU device present");
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
            plugin_info!(logger, "Found CUDA library: {} - GPU likely available", path);
            return true;
        }
    }

    // Check 3: Environment variable set by nvidia-container-toolkit
    if std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok() {
        plugin_info!(logger, "NVIDIA_VISIBLE_DEVICES is set - running in GPU container");
        return true;
    }

    // Check 4: CUDA_VISIBLE_DEVICES (standard CUDA env var)
    if let Ok(val) = std::env::var("CUDA_VISIBLE_DEVICES") {
        // Empty or "-1" means no GPUs
        if !val.is_empty() && val != "-1" {
            plugin_info!(logger, "CUDA_VISIBLE_DEVICES={} - GPU requested", val);
            return true;
        }
    }

    plugin_info!(logger, "No GPU indicators found - assuming CPU-only environment");
    false
}

/// Normalize the execution provider based on actual system capabilities.
/// If user requests CUDA but it's not available, returns "cpu" to match
/// what sherpa-onnx will actually use internally.
fn normalize_execution_provider(logger: &Logger, requested: &str) -> String {
    if requested == "cpu" {
        return "cpu".to_string();
    }

    // For GPU providers, check availability
    if requested == "cuda" || requested == "tensorrt" {
        if is_gpu_available(logger) {
            requested.to_string()
        } else {
            plugin_warn!(
                logger,
                "GPU provider '{}' requested but not available, normalizing to 'cpu' for cache key",
                requested
            );
            "cpu".to_string()
        }
    } else {
        requested.to_string()
    }
}

/// Cached TTS engine with automatic cleanup
struct CachedTtsEngine {
    engine: Arc<TtsEngineWrapper>,
}

/// Wrapper for TTS engine pointer with proper cleanup
struct TtsEngineWrapper {
    engine: *mut ffi::SherpaOnnxOfflineTts,
}

impl TtsEngineWrapper {
    const fn new(engine: *mut ffi::SherpaOnnxOfflineTts) -> Self {
        Self { engine }
    }

    const fn get(&self) -> *mut ffi::SherpaOnnxOfflineTts {
        self.engine
    }
}

unsafe impl Send for TtsEngineWrapper {}
unsafe impl Sync for TtsEngineWrapper {}

impl Drop for TtsEngineWrapper {
    fn drop(&mut self) {
        if !self.engine.is_null() {
            // Note: Can't log here since we don't have access to logger in Drop
            unsafe {
                ffi::SherpaOnnxDestroyOfflineTts(self.engine);
            }
        }
    }
}

/// Global cache of TTS engines
/// Key: (model_dir, num_threads, execution_provider)
// Allow: Type complexity is acceptable here - composite key for caching TTS engines
#[allow(clippy::type_complexity)]
static TTS_ENGINE_CACHE: std::sync::LazyLock<
    Mutex<HashMap<(String, i32, String), CachedTtsEngine>>,
> = std::sync::LazyLock::new(|| {
    // Note: Can't log here since we don't have access to logger in static initialization
    // IMPORTANT: Do NOT warm-load during plugin initialization!
    //
    // Warm loading was causing crashes when CUDA/GPU is enabled because:
    // 1. This Lazy static initializes during plugin load (FFI boundary)
    // 2. ONNX Runtime with CUDA can throw C++ exceptions during initialization
    // 3. Rust's catch_unwind cannot catch foreign (C++) exceptions
    // 4. This causes "fatal runtime error: Rust cannot catch foreign exceptions"
    //
    // Solution: Start with empty cache, engines will be created on first use
    // when we're inside normal Rust code (not FFI boundary).

    Mutex::new(HashMap::new())
});

pub struct KokoroTtsNode {
    tts_engine: Arc<TtsEngineWrapper>,
    config: KokoroTtsConfig,
    text_buffer: String,
    sentence_splitter: SentenceSplitter,
    logger: Logger,
}

// SAFETY: We ensure thread-safety through Arc
unsafe impl Send for KokoroTtsNode {}
unsafe impl Sync for KokoroTtsNode {}

impl NativeProcessorNode for KokoroTtsNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("kokoro")
            .description(
                "High-quality text-to-speech synthesis using the Kokoro TTS model. \
                 Supports 103 voices across Chinese and English with streaming output. \
                 Outputs 24kHz mono audio for real-time playback or further processing.",
            )
            .input("in", &[PacketType::Text])
            .output(
                "out",
                PacketType::RawAudio(AudioFormat {
                    sample_rate: 24000,
                    channels: 1,
                    sample_format: SampleFormat::F32,
                }),
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_dir": {
                        "type": "string",
                        "description": "Path to Kokoro model directory",
                        "default": "./models/kokoro-multi-lang-v1_1"
                    },
                    "speaker_id": {
                        "type": "integer",
                        "description": "Voice ID (0-102 for v1.1)",
                        "default": 50,
                        "minimum": 0,
                        "maximum": 102
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
                    "execution_provider": {
                        "type": "string",
                        "description": "ONNX Runtime execution provider (requires libsherpa-onnx built with GPU support)",
                        "default": "cpu",
                        "enum": ["cpu", "cuda", "tensorrt"]
                    },
                    "emit_telemetry": {
                        "type": "boolean",
                        "description": "Emit out-of-band telemetry events (tts.start/tts.done) to the session telemetry bus",
                        "default": false
                    },
                    "telemetry_preview_chars": {
                        "type": "integer",
                        "description": "Maximum characters of text preview to include in telemetry events (0 = omit preview)",
                        "default": 80,
                        "minimum": 0,
                        "maximum": 1000
                    }
                },
                "required": ["model_dir"]
            }))
            .category("audio")
            .category("tts")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "KokoroTtsNode::new() called with params: {:?}", params);

        let config: KokoroTtsConfig = if let Some(p) = params {
            plugin_info!(
                logger,
                "Parsing config from params: {}",
                serde_json::to_string(&p).unwrap_or_default()
            );
            serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?
        } else {
            plugin_info!(logger, "Using default config (params was None)");
            KokoroTtsConfig::default()
        };

        plugin_info!(
            logger,
            "📋 Config parsed successfully: model_dir={}, speaker_id={}, speed={}, num_threads={}, execution_provider={}",
            config.model_dir,
            config.speaker_id,
            config.speed,
            config.num_threads,
            config.execution_provider
        );

        // Build paths
        plugin_info!(logger, "Building model_dir path");
        let model_dir = PathBuf::from(&config.model_dir);
        let model_dir = if model_dir.is_absolute() {
            plugin_info!(logger, "Model dir is absolute: {}", model_dir.display());
            model_dir
        } else {
            plugin_info!(logger, "Model dir is relative, making absolute");
            std::env::current_dir()
                .map_err(|e| format!("Failed to get current dir: {e}"))?
                .join(model_dir)
        };

        // Canonicalize to resolve symlinks and remove . and .. components
        // This ensures cache keys are consistent regardless of how the path was specified
        let model_dir = model_dir.canonicalize().map_err(|e| {
            format!("Failed to canonicalize model dir '{}': {}", model_dir.display(), e)
        })?;

        plugin_info!(logger, "Final model directory (canonicalized): {}", model_dir.display());

        let model_dir_str = model_dir.to_string_lossy().to_string();

        // Normalize execution provider to match what will actually be loaded
        // This prevents cache poisoning when sherpa-onnx silently falls back to CPU
        let normalized_provider = normalize_execution_provider(&logger, &config.execution_provider);

        // Cache key should only include model-level parameters
        let cache_key = (model_dir_str.clone(), config.num_threads, normalized_provider);

        plugin_info!(logger,
            cache_key_model_dir = %cache_key.0,
            cache_key_threads = cache_key.1,
            cache_key_provider = %cache_key.2,
            "Cache key computed"
        );
        plugin_info!(
            logger,
            "🔍 CACHE KEY: dir='{}' threads={} provider='{}'",
            cache_key.0,
            cache_key.1,
            cache_key.2
        );
        plugin_info!(logger, "Checking TTS engine cache");

        // Check cache first - check if we have a cached engine
        let cached_engine = {
            let cache =
                TTS_ENGINE_CACHE.lock().map_err(|e| format!("Failed to lock TTS cache: {e}"))?;

            plugin_info!(logger,
                cache_size = cache.len(),
                cache_keys = ?cache.keys().collect::<Vec<_>>(),
                "Current cache state before lookup"
            );
            plugin_info!(
                logger,
                "📦 CACHE STATE: size={}, keys={:?}",
                cache.len(),
                cache.keys().collect::<Vec<_>>()
            );

            cache.get(&cache_key).map(|cached| cached.engine.clone())
        };

        let tts_engine = if let Some(engine) = cached_engine {
            plugin_info!(logger,
                model_dir = %model_dir_str,
                "✅ CACHE HIT: Reusing cached TTS engine"
            );
            plugin_info!(logger, "✅ CACHE HIT!");
            engine
        } else {
            plugin_warn!(logger,
                model_dir = %model_dir_str,
                execution_provider = %config.execution_provider,
                "❌ CACHE MISS: Creating new TTS engine (this will take ~5 seconds)"
            );
            plugin_info!(logger, "❌ CACHE MISS - loading model (5 sec)");

            // Try to create the engine with the requested execution provider
            let engine_result = unsafe { create_tts_engine(&logger, &model_dir, &config) };

            let engine_ptr = match engine_result {
                Ok(e) => e,
                Err(e) if config.execution_provider != "cpu" => {
                    // GPU initialization failed - try falling back to CPU
                    plugin_error!(
                        logger,
                        "Failed to create TTS engine with GPU provider '{}': {}",
                        config.execution_provider,
                        e
                    );
                    plugin_warn!(logger, "Attempting fallback to CPU execution provider");

                    // Allow: Clone is required - config is used later for min_sentence_length
                    #[allow(clippy::redundant_clone)]
                    let mut cpu_config = config.clone();
                    cpu_config.execution_provider = "cpu".to_string();

                    match unsafe { create_tts_engine(&logger, &model_dir, &cpu_config) } {
                        Ok(e) => {
                            plugin_info!(
                                logger,
                                "Successfully fell back to CPU execution provider"
                            );
                            e
                        },
                        Err(cpu_err) => {
                            return Err(format!(
                                "GPU init failed: {e}. CPU fallback also failed: {cpu_err}"
                            ));
                        },
                    }
                },
                Err(e) => return Err(e),
            };

            let engine_arc = Arc::new(TtsEngineWrapper::new(engine_ptr));

            // Insert into cache (requires new lock)
            let cache_size = {
                let mut cache = TTS_ENGINE_CACHE
                    .lock()
                    .map_err(|e| format!("Failed to lock TTS cache: {e}"))?;

                // Use the normalized cache key (already computed with normalized provider)
                plugin_info!(logger,
                    cache_key_model_dir = %cache_key.0,
                    cache_key_threads = cache_key.1,
                    cache_key_provider = %cache_key.2,
                    "💾 Inserting TTS engine into cache"
                );
                plugin_info!(
                    logger,
                    "💾 INSERTING into cache: dir='{}' threads={} provider='{}'",
                    cache_key.0,
                    cache_key.1,
                    cache_key.2
                );
                cache.insert(cache_key, CachedTtsEngine { engine: engine_arc.clone() });
                cache.len()
            };

            plugin_info!(logger, new_cache_size = cache_size, "Cache updated");
            plugin_info!(logger, "💾 Cache now has {} entries", cache_size);

            engine_arc
        };

        let min_sentence_length = config.min_sentence_length;

        Ok(Self {
            tts_engine,
            config,
            text_buffer: String::new(),
            sentence_splitter: SentenceSplitter::new(min_sentence_length),
            logger,
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

        plugin_debug!(self.logger, text = %text, "Received text input");

        // Sanitize text before accumulating
        let mut sanitized = Self::sanitize_text(text.as_ref());
        plugin_debug!(self.logger, sanitized = %sanitized, "Sanitized text");

        if sanitized.is_empty() {
            plugin_debug!(self.logger, "Text empty after sanitization, skipping");
            return Ok(());
        }

        // Add sentence-ending punctuation if missing
        if !sanitized.ends_with('.')
            && !sanitized.ends_with('!')
            && !sanitized.ends_with('?')
            && !sanitized.ends_with('。')
            && !sanitized.ends_with('！')
            && !sanitized.ends_with('？')
        {
            sanitized.push('.');
            plugin_debug!(self.logger, "Added sentence-ending punctuation");
        }

        // Accumulate text
        self.text_buffer.push_str(&sanitized);
        plugin_debug!(self.logger, buffer = %self.text_buffer, buffer_len = self.text_buffer.len(), "Updated text buffer");

        // Extract and generate TTS for complete sentences
        while let Some(sentence) = self.sentence_splitter.extract_sentence(&mut self.text_buffer) {
            plugin_info!(self.logger, sentence = %sentence, sentence_len = sentence.len(), "Generating TTS for sentence");
            self.generate_and_send(&sentence, output)?;
        }

        Ok(())
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        if let Some(p) = params {
            let new_config: KokoroTtsConfig =
                serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?;

            // Update mutable parameters
            self.config.speaker_id = new_config.speaker_id;
            self.config.speed = new_config.speed;
        }

        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_info!(
            self.logger,
            buffer_len = self.text_buffer.len(),
            buffer_empty = self.text_buffer.is_empty(),
            "Flush called on Kokoro TTS"
        );

        // Flush any remaining buffered text by processing it as TTS
        if self.text_buffer.is_empty() {
            plugin_info!(
                self.logger,
                "Text buffer was empty during flush - all text was already processed"
            );
        } else {
            let text = self.text_buffer.clone();
            plugin_info!(self.logger,
                remaining_text = %text,
                len = text.len(),
                "Flushing remaining text buffer"
            );
            self.generate_and_send(&text, output)?;
            self.text_buffer.clear();
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        // Buffer should be empty after flush
        if !self.text_buffer.is_empty() {
            plugin_warn!(self.logger,
                remaining_text = %self.text_buffer,
                len = self.text_buffer.len(),
                "Text buffer not empty at cleanup - this shouldn't happen!"
            );
        }
    }
}

impl KokoroTtsNode {
    fn text_preview(&self, text: &str) -> Option<String> {
        let max_chars = self.config.telemetry_preview_chars;
        if max_chars == 0 {
            return None;
        }

        let mut chars = text.chars();
        let prefix: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            Some(format!("{prefix}..."))
        } else {
            Some(prefix)
        }
    }

    fn generate_and_send(&mut self, text: &str, output: &OutputSender) -> Result<(), String> {
        plugin_debug!(self.logger, text_len = text.len(), "Starting TTS generation");

        let start = Instant::now();
        if self.config.emit_telemetry {
            let _ = output.emit_telemetry(
                "tts.start",
                &serde_json::json!({
                    "text_length": text.len(),
                    "text_preview": self.text_preview(text),
                    "speaker_id": self.config.speaker_id,
                    "speed": self.config.speed,
                    "execution_provider": self.config.execution_provider,
                }),
                None,
            );
        }

        let text_cstr = CString::new(text).map_err(|e| format!("Invalid text: {e}"))?;

        // Use non-callback API for best performance
        unsafe {
            let audio_ptr = ffi::SherpaOnnxOfflineTtsGenerate(
                self.tts_engine.get(),
                text_cstr.as_ptr(),
                self.config.speaker_id,
                self.config.speed,
            );

            if audio_ptr.is_null() {
                plugin_error!(self.logger, "TTS generation returned null pointer");
                return Err("TTS generation failed".to_string());
            }

            // Read the generated audio
            let audio = &*audio_ptr;
            if audio.samples.is_null() || audio.n <= 0 {
                plugin_warn!(self.logger, "TTS generated empty audio (null samples or n <= 0)");
                ffi::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio_ptr);
                return Err("TTS generated empty audio".to_string());
            }

            // Allow: Sample count from FFI is guaranteed positive (checked above)
            #[allow(clippy::cast_sign_loss)]
            let sample_count = audio.n as usize;
            plugin_debug!(self.logger, sample_count = sample_count, "TTS generated audio samples");

            let samples = std::slice::from_raw_parts(audio.samples, sample_count);

            // Send all audio at once (simplest, lowest overhead)
            let frame = AudioFrame::new(24000, 1, samples.to_vec());

            plugin_debug!(
                self.logger,
                sample_count = sample_count,
                "Sending audio frame to output"
            );
            output.send("out", &Packet::Audio(frame)).map_err(|e| {
                plugin_error!(self.logger, error = %e, "Failed to send audio frame");
                format!("Failed to send audio: {e}")
            })?;

            plugin_debug!(
                self.logger,
                sample_count = sample_count,
                "Successfully sent audio frame"
            );

            if self.config.emit_telemetry {
                let latency_ms = start.elapsed().as_millis();
                let duration_ms_u64 = u64::try_from(sample_count)
                    .ok()
                    .and_then(|sc| sc.checked_mul(1000))
                    .map_or(0, |num| (num + 12_000) / 24_000);
                let duration_ms = i64::try_from(duration_ms_u64).unwrap_or(i64::MAX);
                let _ = output.emit_telemetry(
                    "tts.done",
                    &serde_json::json!({
                        "text_length": text.len(),
                        "text_preview": self.text_preview(text),
                        "speaker_id": self.config.speaker_id,
                        "speed": self.config.speed,
                        "execution_provider": self.config.execution_provider,
                        "audio_samples": sample_count,
                        "audio_duration_ms": duration_ms,
                        "latency_ms": latency_ms,
                    }),
                    None,
                );
            }

            // Destroy the audio object
            ffi::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio_ptr);
        }

        Ok(())
    }

    fn sanitize_text(text: &str) -> String {
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
                | '\u{4E00}'..='\u{9FFF}'
                | '。'
                | '，'
                | '！'
                | '？'
                | '、'
                | '；'
                | '：'
                | '（'
                | '）' => Some(c),
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
    logger: &Logger,
    model_dir: &Path,
    config: &KokoroTtsConfig,
) -> Result<*mut ffi::SherpaOnnxOfflineTts, String> {
    plugin_info!(logger, "Entering create_tts_engine: model_dir={}", model_dir.display());

    let model_path = model_dir.join("model.onnx");
    let voices_path = model_dir.join("voices.bin");
    let tokens_path = model_dir.join("tokens.txt");
    let data_dir = model_dir.join("espeak-ng-data");
    let dict_dir = model_dir.join("dict");
    let lexicon_us = model_dir.join("lexicon-us-en.txt");
    let lexicon_zh = model_dir.join("lexicon-zh.txt");

    plugin_info!(logger, "Built all paths");

    // Verify files exist
    for (name, path) in [("model", &model_path), ("voices", &voices_path), ("tokens", &tokens_path)]
    {
        if !path.exists() {
            return Err(format!("{} file not found: {}", name, path.display()));
        }
        plugin_info!(logger, file = %path.display(), "File exists: {}", name);
    }

    // Create C strings - keep them alive until after SherpaOnnxCreateOfflineTts call
    plugin_info!(logger, "Creating CStrings for paths");
    let model_cstr = path_to_cstring(&model_path)?;
    let voices_cstr = path_to_cstring(&voices_path)?;
    let tokens_cstr = path_to_cstring(&tokens_path)?;
    let data_dir_cstr = path_to_cstring(&data_dir)?;
    let dict_dir_cstr = path_to_cstring(&dict_dir)?;

    let lexicon = format!("{},{}", lexicon_us.to_string_lossy(), lexicon_zh.to_string_lossy());
    plugin_info!(logger, lexicon = %lexicon, "Built lexicon string");
    let lexicon_cstr = CString::new(lexicon).map_err(|e| format!("Invalid lexicon string: {e}"))?;

    // IMPORTANT: Keep provider CStrings alive
    let provider_cstr = CString::new(config.execution_provider.as_str())
        .map_err(|_| "Invalid execution provider string".to_string())?;

    // Language field for Kokoro (empty = auto-detect)
    let lang_cstr = CString::new("").map_err(|e| format!("Invalid lang string: {e}"))?;

    plugin_info!(logger, "All CStrings created, building config struct");

    // Build config - match exact C API struct layout!
    let tts_config = ffi::SherpaOnnxOfflineTtsConfig {
        model: ffi::SherpaOnnxOfflineTtsModelConfig {
            // VITS comes first in C API
            vits: ffi::SherpaOnnxOfflineTtsVitsModelConfig {
                model: ptr::null(),
                lexicon: ptr::null(),
                tokens: ptr::null(),
                data_dir: ptr::null(),
                noise_scale: 0.0,
                noise_scale_w: 0.0,
                length_scale: 1.0,
                dict_dir: ptr::null(),
            },
            // Common model config fields
            num_threads: config.num_threads,
            debug: 0,
            provider: provider_cstr.as_ptr(),
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
            // Kokoro config (what we actually use)
            kokoro: ffi::SherpaOnnxOfflineTtsKokoroModelConfig {
                model: model_cstr.as_ptr(),
                voices: voices_cstr.as_ptr(),
                tokens: tokens_cstr.as_ptr(),
                data_dir: data_dir_cstr.as_ptr(),
                length_scale: 1.0,
                dict_dir: dict_dir_cstr.as_ptr(),
                lexicon: lexicon_cstr.as_ptr(),
                lang: lang_cstr.as_ptr(),
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
        rule_fsts: lang_cstr.as_ptr(),
        max_num_sentences: 1,
        rule_fars: lang_cstr.as_ptr(),
        silence_scale: 1.0,
    };

    plugin_info!(logger,
        model = %model_path.display(),
        voices = %voices_path.display(),
        tokens = %tokens_path.display(),
        num_threads = config.num_threads,
        execution_provider = %config.execution_provider,
        "About to call SherpaOnnxCreateOfflineTts with CUDA"
    );

    plugin_warn!(logger, "=== CRITICAL: Calling ONNX Runtime with CUDA provider ===");
    plugin_warn!(logger, "If this crashes with 'foreign exception', it means:");
    plugin_warn!(logger, "1. ONNX Runtime threw a C++ exception");
    plugin_warn!(logger, "2. Likely cause: CUDA provider not available in this ONNX Runtime build");
    plugin_warn!(logger, "3. Or: CUDA version mismatch");

    let tts = ffi::SherpaOnnxCreateOfflineTts(&raw const tts_config);

    plugin_info!(logger, "✓ SherpaOnnxCreateOfflineTts succeeded: ptr={:p}", tts);

    if tts.is_null() {
        return Err("Failed to create TTS engine".to_string());
    }

    plugin_info!(logger, "TTS engine created successfully");
    Ok(tts)
}

fn path_to_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| format!("Invalid path: {e}"))
}

impl Drop for KokoroTtsNode {
    fn drop(&mut self) {
        // Arc reference will be dropped automatically
        // When the last Arc reference is dropped, TtsEngineWrapper::drop() will clean up the engine
    }
}
