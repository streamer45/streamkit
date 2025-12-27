// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A native plugin for real-time speech-to-text using Whisper with Silero VAD
//!
//! This plugin provides high-performance CPU-based transcription with VAD-based
//! segmentation for natural speech boundaries and zero chunking artifacts.

mod vad;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    AudioFormat, SampleFormat, TranscriptionData, TranscriptionSegment,
};
use vad::SileroVAD;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

#[derive(Serialize, Deserialize, Clone, Default)]
struct WhisperGpuConfig {
    /// Enable GPU acceleration (requires CUDA support in whisper.cpp)
    #[serde(default)]
    use_gpu: bool,

    /// GPU device ID (0 = first GPU, 1 = second GPU, etc.)
    #[serde(default)]
    gpu_device: i32,
}

#[derive(Serialize, Deserialize, Clone)]
struct WhisperSuppressionConfig {
    /// Suppress blank audio segments (default: true)
    #[serde(default = "default_suppress_blank")]
    suppress_blank: bool,

    /// Suppress non-speech tokens like [BLANK_AUDIO], [MUSIC], etc. (default: true)
    #[serde(default = "default_suppress_non_speech_tokens")]
    suppress_non_speech_tokens: bool,
}

impl Default for WhisperSuppressionConfig {
    fn default() -> Self {
        Self {
            suppress_blank: default_suppress_blank(),
            suppress_non_speech_tokens: default_suppress_non_speech_tokens(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct WhisperTelemetryConfig {
    /// Emit VAD speech start/end out-of-band to the session telemetry bus.
    ///
    /// These events do not flow through graph pins.
    #[serde(default)]
    emit_vad_events: bool,
}

/// Configuration for the Whisper STT plugin with VAD
#[derive(Serialize, Deserialize, Clone)]
struct WhisperConfig {
    /// Path to the Whisper GGML model file
    #[serde(default = "default_model_path")]
    model_path: String,

    /// Language code (e.g., "en", "es", "fr")
    #[serde(default = "default_language")]
    language: String,

    /// Path to the Silero VAD ONNX model file
    #[serde(default = "default_vad_model_path")]
    vad_model_path: String,

    /// VAD speech probability threshold (0.0-1.0)
    #[serde(default = "default_vad_threshold")]
    vad_threshold: f32,

    /// Minimum silence duration before triggering transcription (milliseconds)
    #[serde(default = "default_min_silence_duration_ms")]
    min_silence_duration_ms: u64,

    /// Maximum segment duration before forcing transcription (seconds)
    #[serde(default = "default_max_segment_duration_secs")]
    max_segment_duration_secs: f32,

    /// Number of threads to use for decoding (0 = auto: min(4, num_cores))
    #[serde(default = "default_n_threads")]
    n_threads: usize,

    #[serde(flatten)]
    gpu: WhisperGpuConfig,

    #[serde(flatten)]
    suppression: WhisperSuppressionConfig,

    #[serde(flatten)]
    telemetry: WhisperTelemetryConfig,
}

const fn default_suppress_blank() -> bool {
    true
}

const fn default_suppress_non_speech_tokens() -> bool {
    true
}

fn default_model_path() -> String {
    "models/ggml-base.en-q5_1.bin".to_string()
}

fn default_language() -> String {
    "en".to_string()
}

fn default_vad_model_path() -> String {
    "models/silero_vad.onnx".to_string()
}

const fn default_vad_threshold() -> f32 {
    0.5
}

const fn default_min_silence_duration_ms() -> u64 {
    700
}

const fn default_max_segment_duration_secs() -> f32 {
    30.0
}

const fn default_n_threads() -> usize {
    0 // 0 = use whisper.cpp default (min(4, num_cores))
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            language: default_language(),
            vad_model_path: default_vad_model_path(),
            vad_threshold: default_vad_threshold(),
            min_silence_duration_ms: default_min_silence_duration_ms(),
            max_segment_duration_secs: default_max_segment_duration_secs(),
            n_threads: default_n_threads(),
            gpu: WhisperGpuConfig::default(),
            suppression: WhisperSuppressionConfig::default(),
            telemetry: WhisperTelemetryConfig::default(),
        }
    }
}

/// Wrapper for cached Whisper contexts
/// We cache WhisperContext (the model) but NOT WhisperState (per-instance state)
#[derive(Clone)]
struct CachedWhisperContext {
    context: Arc<WhisperContext>,
}

unsafe impl Send for CachedWhisperContext {}
unsafe impl Sync for CachedWhisperContext {}

/// Global cache of Whisper contexts
/// Key: (model_path, use_gpu, gpu_device)
// Allow: Type complexity is acceptable here - this is a cache with a composite key
// that needs to match model loading parameters. Splitting into a named type would
// reduce clarity since this is the only place the key is used.
#[allow(clippy::type_complexity)]
static WHISPER_CONTEXT_CACHE: std::sync::LazyLock<
    Mutex<HashMap<(String, bool, i32), CachedWhisperContext>>,
> = std::sync::LazyLock::new(|| {
    tracing::info!("Initializing Whisper context cache");
    Mutex::new(HashMap::new())
});

/// Validate that audio format meets Whisper's requirements (16kHz mono f32)
fn validate_audio_format(sample_rate: u32, channels: u16) -> Result<(), String> {
    if sample_rate != 16000 {
        return Err(format!(
            "Whisper requires 16kHz audio, got {sample_rate}Hz. Please add an audio_resample node upstream."
        ));
    }

    if channels != 1 {
        return Err(format!(
            "Whisper requires mono audio, got {channels} channels. Please add an audio_resample node upstream."
        ));
    }

    Ok(())
}

/// The Whisper STT plugin with VAD-based segmentation
pub struct WhisperPlugin {
    config: WhisperConfig,
    whisper_context: Arc<WhisperContext>,
    whisper_state: WhisperState,
    vad: SileroVAD,

    // Frame buffering (for VAD)
    frame_buffer: VecDeque<f32>,

    // Speech segment buffering (for Whisper)
    speech_buffer: VecDeque<f32>,
    segment_start_time_ms: u64,
    segment_counter: u64,
    current_segment_id: Option<String>,

    // Silence tracking
    silence_frame_count: usize,
    silence_threshold_frames: usize,

    // Time tracking
    absolute_time_ms: u64,
}

impl NativeProcessorNode for WhisperPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("whisper")
            .description(
                "Real-time speech-to-text transcription using OpenAI's Whisper model. \
                 Features VAD-based segmentation for natural speech boundaries, \
                 GPU acceleration support, and streaming output. \
                 Requires 16kHz mono audio input.",
            )
            .input(
                "in",
                &[PacketType::RawAudio(AudioFormat {
                    sample_rate: 16000, // Requires 16kHz
                    channels: 1,        // Requires mono
                    sample_format: SampleFormat::F32,
                })],
            )
            .output("out", PacketType::Transcription)
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_path": {
                        "type": "string",
                        "description": "Path to Whisper GGML model file (relative to repo root). IMPORTANT: Input audio must be 16kHz mono f32.",
                        "default": "models/ggml-base.en-q5_1.bin"
                    },
                    "language": {
                        "type": "string",
                        "description": "Language code (e.g., 'en', 'es', 'fr')",
                        "default": "en"
                    },
                    "vad_model_path": {
                        "type": "string",
                        "description": "Path to Silero VAD ONNX model file",
                        "default": "models/silero_vad.onnx"
                    },
                    "vad_threshold": {
                        "type": "number",
                        "description": "VAD speech probability threshold (0.0-1.0)",
                        "default": 0.5,
                        "minimum": 0.0,
                        "maximum": 1.0
                    },
                    "min_silence_duration_ms": {
                        "type": "integer",
                        "description": "Minimum silence duration before transcription (milliseconds)",
                        "default": 700,
                        "minimum": 100,
                        "maximum": 5000
                    },
                    "max_segment_duration_secs": {
                        "type": "number",
                        "description": "Maximum segment duration before forced transcription (seconds)",
                        "default": 30.0,
                        "minimum": 5.0,
                        "maximum": 120.0
                    },
                    "n_threads": {
                        "type": "integer",
                        "description": "Number of threads for decoding (0 = auto: min(4, num_cores), 8-12 recommended for modern CPUs)",
                        "default": 0,
                        "minimum": 0,
                        "maximum": 32
                    },
                    "use_gpu": {
                        "type": "boolean",
                        "description": "Enable GPU acceleration (requires whisper.cpp built with CUDA support)",
                        "default": false
                    },
                    "gpu_device": {
                        "type": "integer",
                        "description": "GPU device ID to use (0 = first GPU, 1 = second GPU, etc.)",
                        "default": 0,
                        "minimum": 0,
                        "maximum": 7
                    },
                    "suppress_blank": {
                        "type": "boolean",
                        "description": "Suppress blank/silent audio segments",
                        "default": true
                    },
                    "suppress_non_speech_tokens": {
                        "type": "boolean",
                        "description": "Suppress non-speech tokens like [BLANK_AUDIO], [MUSIC], [APPLAUSE], etc.",
                        "default": true
                    },
                    "emit_vad_events": {
                        "type": "boolean",
                        "description": "Emit VAD speech start/end out-of-band to the telemetry bus (does not flow through graph pins).",
                        "default": false
                    }
                }
            }))
            .category("ml")
            .category("speech")
            .category("transcription")
            .build()
    }

    fn new(params: Option<Value>, _logger: Logger) -> Result<Self, String> {
        let config: WhisperConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Invalid config: {e}"))?
        } else {
            WhisperConfig::default()
        };

        // Cache key: only model-level parameters (model_path, GPU settings)
        let cache_key = (config.model_path.clone(), config.gpu.use_gpu, config.gpu.gpu_device);

        // Get or create cached Whisper context
        let whisper_context = {
            let mut cache = WHISPER_CONTEXT_CACHE
                .lock()
                .map_err(|e| format!("Failed to lock Whisper cache: {e}"))?;

            if let Some(cached) = cache.get(&cache_key) {
                tracing::info!(
                    model_path = %config.model_path,
                    use_gpu = config.gpu.use_gpu,
                    "✅ CACHE HIT: Reusing cached Whisper context"
                );
                cached.context.clone()
            } else {
                tracing::info!(
                    model_path = %config.model_path,
                    use_gpu = config.gpu.use_gpu,
                    gpu_device = config.gpu.gpu_device,
                    "❌ CACHE MISS: Loading Whisper model (this will take several seconds)"
                );

                // Load Whisper model
                let mut whisper_params = WhisperContextParameters::default();
                if config.gpu.use_gpu {
                    whisper_params.use_gpu = true;
                    whisper_params.gpu_device = config.gpu.gpu_device;
                }

                let context = WhisperContext::new_with_params(&config.model_path, whisper_params)
                    .map_err(|e| {
                    format!("Failed to load Whisper model from '{}': {}", config.model_path, e)
                })?;

                let context_arc = Arc::new(context);

                // Cache for future use
                cache.insert(cache_key, CachedWhisperContext { context: context_arc.clone() });

                tracing::info!("✅ Whisper model loaded and cached");
                drop(cache); // Release lock early
                context_arc
            }
        };

        // Create per-instance Whisper state (NOT cached - each instance needs its own)
        let whisper_state = whisper_context
            .create_state()
            .map_err(|e| format!("Failed to create Whisper state: {e}"))?;

        // Initialize Silero VAD
        let vad = SileroVAD::new(&config.vad_model_path, 16000, config.vad_threshold)
            .map_err(|e| format!("Failed to initialize VAD: {e}"))?;

        // Calculate silence threshold in frames (each frame is 32ms)
        let silence_threshold_frames = (config.min_silence_duration_ms / 32) as usize;

        Ok(Self {
            config,
            whisper_context,
            whisper_state,
            vad,
            frame_buffer: VecDeque::with_capacity(1024),
            speech_buffer: VecDeque::with_capacity(16000 * 30), // 30 seconds max
            segment_start_time_ms: 0,
            segment_counter: 0,
            current_segment_id: None,
            silence_frame_count: 0,
            silence_threshold_frames,
            absolute_time_ms: 0,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Audio(frame) => {
                // Validate audio format (must be 16kHz mono f32)
                validate_audio_format(frame.sample_rate, frame.channels)?;

                // Add samples to frame buffer (Arc derefs to slice)
                self.frame_buffer.extend(frame.samples.as_ref().as_slice().iter().copied());

                // Process complete 512-sample frames through VAD
                while self.frame_buffer.len() >= 512 {
                    let vad_frame: Vec<f32> = self.frame_buffer.drain(..512).collect();

                    let probability = self
                        .vad
                        .process_chunk(&vad_frame)
                        .map_err(|e| format!("VAD processing failed: {e}"))?;
                    let is_speech = probability >= self.config.vad_threshold;

                    if is_speech {
                        // Speech detected
                        self.silence_frame_count = 0;

                        // Start new segment if needed
                        if self.speech_buffer.is_empty() {
                            self.segment_start_time_ms = self.absolute_time_ms;
                            self.segment_counter = self.segment_counter.saturating_add(1);
                            self.current_segment_id = Some(format!(
                                "seg-{}-{}",
                                self.segment_start_time_ms, self.segment_counter
                            ));

                            if self.config.telemetry.emit_vad_events {
                                if let Some(segment_id) = self.current_segment_id.clone() {
                                    let _ = output.emit_telemetry(
                                        "vad.speech_start",
                                        &serde_json::json!({
                                            "segment_id": segment_id,
                                            "start_time_ms": self.segment_start_time_ms,
                                            "speech_probability": probability,
                                            "threshold": self.config.vad_threshold,
                                        }),
                                        None,
                                    );
                                }
                            }
                        }

                        // Add to speech buffer
                        self.speech_buffer.extend(&vad_frame);

                        // Check max duration
                        let segment_duration_ms =
                            self.absolute_time_ms - self.segment_start_time_ms;
                        // Allow: Config value is always positive, cast to u64 for duration comparison
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let max_duration_ms =
                            (self.config.max_segment_duration_secs * 1000.0) as u64;
                        if segment_duration_ms >= max_duration_ms {
                            let end_time_ms = self.absolute_time_ms.saturating_add(32);
                            self.transcribe_and_emit(output, end_time_ms, "max_duration", None)?;
                        }
                    } else {
                        // Silence detected
                        self.silence_frame_count += 1;

                        // Check if we have buffered speech and enough silence
                        if !self.speech_buffer.is_empty()
                            && self.silence_frame_count >= self.silence_threshold_frames
                        {
                            let silence_frames = self.silence_frame_count.saturating_sub(1) as u64;
                            let end_time_ms =
                                self.absolute_time_ms.saturating_sub(silence_frames * 32);
                            let silence_duration_ms = Some((self.silence_frame_count as u64) * 32);
                            self.transcribe_and_emit(
                                output,
                                end_time_ms,
                                "silence",
                                silence_duration_ms,
                            )?;
                        }
                    }

                    self.absolute_time_ms += 32; // 512 samples @ 16kHz = 32ms
                }

                Ok(())
            },
            _ => Err("Whisper plugin only accepts audio packets".to_string()),
        }
    }

    fn update_params(&mut self, params: Option<Value>) -> Result<(), String> {
        if let Some(p) = params {
            let new_config: WhisperConfig =
                serde_json::from_value(p).map_err(|e| format!("Invalid config: {e}"))?;

            // If Whisper model path or GPU settings changed, get new cached context
            if new_config.model_path != self.config.model_path
                || new_config.gpu.use_gpu != self.config.gpu.use_gpu
                || new_config.gpu.gpu_device != self.config.gpu.gpu_device
            {
                let cache_key = (
                    new_config.model_path.clone(),
                    new_config.gpu.use_gpu,
                    new_config.gpu.gpu_device,
                );

                let whisper_context = {
                    let mut cache = WHISPER_CONTEXT_CACHE
                        .lock()
                        .map_err(|e| format!("Failed to lock Whisper cache: {e}"))?;

                    if let Some(cached) = cache.get(&cache_key) {
                        tracing::info!("Reusing cached Whisper context for updated params");
                        cached.context.clone()
                    } else {
                        tracing::info!("Loading new Whisper context for updated params");

                        let mut whisper_params = WhisperContextParameters::default();
                        if new_config.gpu.use_gpu {
                            whisper_params.use_gpu = true;
                            whisper_params.gpu_device = new_config.gpu.gpu_device;
                        }

                        let context =
                            WhisperContext::new_with_params(&new_config.model_path, whisper_params)
                                .map_err(|e| format!("Failed to reload Whisper model: {e}"))?;

                        let context_arc = Arc::new(context);

                        cache.insert(
                            cache_key,
                            CachedWhisperContext { context: context_arc.clone() },
                        );

                        context_arc
                    }
                };

                self.whisper_context = whisper_context;

                // Recreate state with new context
                self.whisper_state = self
                    .whisper_context
                    .create_state()
                    .map_err(|e| format!("Failed to recreate Whisper state: {e}"))?;
            }

            // If VAD model path changed, reload VAD
            // Allow: Threshold is a config value that changes infrequently, exact comparison is appropriate
            #[allow(clippy::float_cmp)]
            let threshold_changed = new_config.vad_threshold != self.config.vad_threshold;

            if new_config.vad_model_path != self.config.vad_model_path || threshold_changed {
                self.vad =
                    SileroVAD::new(&new_config.vad_model_path, 16000, new_config.vad_threshold)
                        .map_err(|e| format!("Failed to reload VAD: {e}"))?;
            }

            // Update VAD threshold
            if threshold_changed {
                self.vad.set_threshold(new_config.vad_threshold);
            }

            // Update silence threshold frames
            if new_config.min_silence_duration_ms != self.config.min_silence_duration_ms {
                self.silence_threshold_frames = (new_config.min_silence_duration_ms / 32) as usize;
            }

            self.config = new_config;
        }
        Ok(())
    }
}

impl WhisperPlugin {
    /// Transcribe buffered speech segment and emit result
    fn transcribe_and_emit(
        &mut self,
        output: &OutputSender,
        end_time_ms: u64,
        reason: &'static str,
        silence_duration_ms: Option<u64>,
    ) -> Result<(), String> {
        if self.speech_buffer.is_empty() {
            return Ok(());
        }

        if self.config.telemetry.emit_vad_events {
            if let Some(segment_id) = self.current_segment_id.take() {
                let duration_ms = end_time_ms.saturating_sub(self.segment_start_time_ms);
                let _ = output.emit_telemetry(
                    "vad.speech_end",
                    &serde_json::json!({
                        "segment_id": segment_id,
                        "start_time_ms": self.segment_start_time_ms,
                        "end_time_ms": end_time_ms,
                        "duration_ms": duration_ms,
                        "reason": reason,
                        "silence_duration_ms": silence_duration_ms,
                    }),
                    None,
                );
            }
        }

        // Collect speech samples
        let samples: Vec<f32> = self.speech_buffer.drain(..).collect();

        // Allow: Loss of precision acceptable for logging, audio segments are never large enough to matter
        #[allow(clippy::cast_precision_loss)]
        let duration_secs = samples.len() as f32 / 16000.0;
        tracing::debug!(
            samples = samples.len(),
            duration_secs = duration_secs,
            "Transcribing speech segment"
        );

        // Configure Whisper parameters
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.config.language));
        params.set_translate(false);
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // Suppress blank segments and non-speech tokens (e.g., [BLANK_AUDIO], [MUSIC])
        params.set_suppress_blank(self.config.suppression.suppress_blank);
        params.set_suppress_nst(self.config.suppression.suppress_non_speech_tokens);

        // Set thread count if configured (0 = use whisper.cpp default)
        if self.config.n_threads > 0 {
            // Allow: Thread count is bounded by system CPUs, truncation/wrap is not a concern
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            params.set_n_threads(self.config.n_threads as i32);
        }

        // Run Whisper inference
        self.whisper_state
            .full(params, &samples)
            .map_err(|e| format!("Whisper inference failed: {e}"))?;

        // Collect segments with absolute timestamps
        let mut segments = Vec::new();
        for segment in self.whisper_state.as_iter() {
            match segment.to_str() {
                Ok(text) => {
                    let text_trimmed = text.trim();
                    if !text_trimmed.is_empty() {
                        // Whisper returns timestamps in centiseconds (10ms units) relative to segment
                        // Allow: Timestamps are always positive (audio duration), safe to cast to u64
                        #[allow(clippy::cast_sign_loss)]
                        let segment_relative_start_ms = (segment.start_timestamp() * 10) as u64;
                        #[allow(clippy::cast_sign_loss)]
                        let segment_relative_end_ms = (segment.end_timestamp() * 10) as u64;

                        let start_time_ms = self.segment_start_time_ms + segment_relative_start_ms;
                        let end_time_ms = self.segment_start_time_ms + segment_relative_end_ms;

                        segments.push(TranscriptionSegment {
                            text: text_trimmed.to_string(),
                            start_time_ms,
                            end_time_ms,
                            confidence: None, // Whisper doesn't provide confidence scores
                        });
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to get segment text: {}", e);
                },
            }
        }

        // Emit transcription if we have segments
        if segments.is_empty() {
            tracing::warn!("Whisper inference produced no segments");
        } else {
            let full_text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");

            tracing::info!(text = %full_text, "Transcription");

            output.send(
                "out",
                &Packet::Transcription(std::sync::Arc::new(TranscriptionData {
                    text: full_text,
                    segments,
                    language: Some(self.config.language.clone()),
                    metadata: None,
                })),
            )?;
        }

        // Reset for next segment
        self.silence_frame_count = 0;

        Ok(())
    }
}

// Export the plugin entry point
native_plugin_entry!(WhisperPlugin);
