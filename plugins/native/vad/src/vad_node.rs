// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Main VAD node implementation

use crate::config::{vad_cache_key, VadConfig, VadOutputMode};
use crate::ffi;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    AudioFormat, CustomEncoding, CustomPacketData, PacketMetadata, SampleFormat,
};

const VAD_EVENT_TYPE_ID: &str = "plugin::native::vad/vad-event@1";

/// Cached VAD detector wrapper
struct CachedVadDetector {
    detector: *mut ffi::SherpaOnnxVoiceActivityDetector,
}

unsafe impl Send for CachedVadDetector {}
unsafe impl Sync for CachedVadDetector {}

impl Drop for CachedVadDetector {
    fn drop(&mut self) {
        if !self.detector.is_null() {
            unsafe {
                ffi::SherpaOnnxDestroyVoiceActivityDetector(self.detector);
            }
        }
    }
}

/// Global cache for VAD detectors (keyed by model path, threads, provider)
static VAD_CACHE: std::sync::LazyLock<Mutex<HashMap<String, Arc<CachedVadDetector>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Speech state for tracking transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeechState {
    Idle,
    Speaking,
}

/// VAD plugin node
pub struct VadNode {
    /// Shared VAD detector
    detector: Arc<CachedVadDetector>,
    /// Plugin configuration
    config: VadConfig,
    /// Current speech state
    speech_state: SpeechState,
    /// Timestamp when current speech segment started (ms)
    speech_start_ms: Option<u64>,
    /// Current absolute time in milliseconds
    absolute_time_ms: u64,
    /// Logger
    logger: Logger,
}

impl VadNode {
    /// Create a new VAD detector from config
    fn create_detector(
        config: &VadConfig,
        logger: &Logger,
    ) -> Result<*mut ffi::SherpaOnnxVoiceActivityDetector, String> {
        plugin_info!(logger, "Initializing ten-vad detector");

        // Convert strings to C strings
        let model_path = CString::new(config.model_path.as_str())
            .map_err(|e| format!("Invalid model path: {}", e))?;
        let provider = CString::new(config.provider.as_str())
            .map_err(|e| format!("Invalid provider: {}", e))?;
        let empty_string = CString::new("").unwrap();

        // Configure ten-vad
        let ten_vad_config = ffi::SherpaOnnxTenVadModelConfig {
            model: model_path.as_ptr(),
            threshold: config.threshold,
            min_silence_duration: config.min_silence_duration_s,
            min_speech_duration: config.min_speech_duration_s,
            window_size: config.window_size,
            max_speech_duration: config.max_speech_duration_s,
        };

        // Empty Silero config (not used)
        let silero_vad_config = ffi::SherpaOnnxSileroVadModelConfig {
            model: empty_string.as_ptr(),
            threshold: 0.5,
            min_silence_duration: 0.5,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 30.0,
        };

        // Overall VAD config
        let vad_config = ffi::SherpaOnnxVadModelConfig {
            silero_vad: silero_vad_config,
            sample_rate: 16000,
            num_threads: config.num_threads,
            provider: provider.as_ptr(),
            debug: if config.debug { 1 } else { 0 },
            ten_vad: ten_vad_config,
        };

        // Create detector with 30 second buffer
        let detector = unsafe { ffi::SherpaOnnxCreateVoiceActivityDetector(&vad_config, 30.0) };

        if detector.is_null() {
            return Err("Failed to create VAD detector".to_string());
        }

        plugin_info!(
            logger,
            model = %config.model_path,
            threshold = config.threshold,
            "VAD detector created successfully"
        );

        Ok(detector)
    }

    /// Emit a custom VAD event (JSON payload).
    fn emit_event(
        &self,
        event_type: &'static str,
        timestamp_ms: u64,
        duration_ms: Option<u64>,
        output: &OutputSender,
    ) -> Result<(), String> {
        let data = serde_json::json!({
            "event_type": event_type,
            "timestamp_ms": timestamp_ms,
            "duration_ms": duration_ms
        });

        let packet = Packet::Custom(Arc::new(CustomPacketData {
            type_id: VAD_EVENT_TYPE_ID.to_string(),
            encoding: CustomEncoding::Json,
            data,
            metadata: Some(PacketMetadata {
                timestamp_us: Some(timestamp_ms.saturating_mul(1000)),
                duration_us: None,
                sequence: None,
            }),
        }));

        plugin_debug!(
            self.logger,
            type_id = VAD_EVENT_TYPE_ID,
            event_type = event_type,
            timestamp_ms = timestamp_ms,
            "Emitted custom VAD event"
        );

        // For linear pipelines (`steps:`), only the `out` pin is connected by default.
        // Emitting on another pin would drop the packet (and warn) unless the user explicitly
        // wires that pin in a dynamic pipeline.
        output.send("out", &packet)?;

        Ok(())
    }

    /// Process detected speech segments from the queue (FilteredAudio mode only)
    fn process_segments(&mut self, output: &OutputSender) -> Result<(), String> {
        // Only process segments for FilteredAudio mode
        if self.config.output_mode != VadOutputMode::FilteredAudio {
            return Ok(());
        }

        unsafe {
            while ffi::SherpaOnnxVoiceActivityDetectorEmpty(self.detector.detector) == 0 {
                // Get the first segment
                let segment_ptr = ffi::SherpaOnnxVoiceActivityDetectorFront(self.detector.detector);
                if segment_ptr.is_null() {
                    break;
                }

                let segment = &*segment_ptr;
                let sample_count = segment.n as usize;

                if sample_count > 0 {
                    // Copy samples
                    let samples = std::slice::from_raw_parts(segment.samples, sample_count);
                    let samples_vec = samples.to_vec();

                    // Calculate segment duration
                    let segment_duration_ms = (sample_count as f64 / 16000.0 * 1000.0) as u64;

                    // Emit the audio segment
                    output.send("out", &Packet::Audio(AudioFrame::new(16000, 1, samples_vec)))?;

                    plugin_trace!(
                        self.logger,
                        samples = sample_count,
                        duration_ms = segment_duration_ms,
                        "Emitted speech segment"
                    );
                }

                // Pop the segment from the queue
                ffi::SherpaOnnxVoiceActivityDetectorPop(self.detector.detector);
                ffi::SherpaOnnxDestroySpeechSegment(segment_ptr);
            }
        }

        Ok(())
    }
}

impl NativeProcessorNode for VadNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("vad")
            .description(
                "Voice Activity Detection (VAD) using a high-performance ONNX model. \
                 Can output speech/silence events for downstream processing, or filter audio \
                 to pass only speech segments. Requires 16kHz mono audio input.",
            )
            .input(
                "in",
                &[PacketType::RawAudio(AudioFormat {
                    sample_rate: 16000,
                    channels: 1,
                    sample_format: SampleFormat::F32,
                })],
            )
            // Output type depends on `output_mode`:
            // - `events`: `Custom` packets (type_id: plugin::native::vad/vad-event@1)
            // - `filtered_audio`: `RawAudio` speech segments (16kHz mono f32)
            .output("out", PacketType::Any)
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_path": {
                        "type": "string",
                        "description": "Path to the ten-vad ONNX model",
                        "default": "models/ten-vad.onnx"
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["events", "filtered_audio"],
                        "description": "Output mode: 'events' emits Custom packets on 'out' (type_id: plugin::native::vad/vad-event@1), 'filtered_audio' emits speech segments on 'out'",
                        "default": "events"
                    },
                    "threshold": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "VAD threshold (higher = more conservative)",
                        "default": 0.5
                    },
                    "min_silence_duration_s": {
                        "type": "number",
                        "description": "Minimum silence duration in seconds to trigger speech end",
                        "default": 0.5
                    },
                    "min_speech_duration_s": {
                        "type": "number",
                        "description": "Minimum speech duration in seconds",
                        "default": 0.25
                    },
                    "window_size": {
                        "type": "integer",
                        "description": "Window size for VAD processing (samples)",
                        "default": 512
                    },
                    "max_speech_duration_s": {
                        "type": "number",
                        "description": "Maximum speech duration in seconds",
                        "default": 30.0
                    },
                    "num_threads": {
                        "type": "integer",
                        "description": "Number of threads for ONNX runtime",
                        "default": 1
                    },
                    "provider": {
                        "type": "string",
                        "description": "ONNX execution provider (cpu, cuda, etc.)",
                        "default": "cpu"
                    },
                    "debug": {
                        "type": "boolean",
                        "description": "Enable debug logging from sherpa-onnx",
                        "default": false
                    }
                }
            }))
            .category("audio")
            .category("ml")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "Initializing VAD plugin");

        // Parse configuration
        let config: VadConfig = if let Some(params) = params {
            serde_json::from_value(params).map_err(|e| format!("Invalid configuration: {}", e))?
        } else {
            VadConfig::default()
        };

        plugin_debug!(logger, config = ?config, "Parsed VAD configuration");

        // Get or create cached detector
        let cache_key = vad_cache_key(&config);
        let detector = {
            let mut cache = VAD_CACHE.lock().unwrap();

            if let Some(cached) = cache.get(&cache_key) {
                plugin_info!(logger, "✅ CACHE HIT: Reusing cached VAD detector");
                cached.clone()
            } else {
                plugin_info!(logger, "❌ CACHE MISS: Creating new VAD detector");
                let detector_ptr = Self::create_detector(&config, &logger)?;
                let cached = Arc::new(CachedVadDetector { detector: detector_ptr });
                cache.insert(cache_key, cached.clone());
                cached
            }
        };

        Ok(Self {
            detector,
            config,
            speech_state: SpeechState::Idle,
            speech_start_ms: None,
            absolute_time_ms: 0,
            logger,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Audio(frame) => {
                // Validate audio format
                if frame.sample_rate != 16000 {
                    return Err(format!("VAD requires 16kHz audio, got {}Hz", frame.sample_rate));
                }
                if frame.channels != 1 {
                    return Err(format!(
                        "VAD requires mono audio, got {} channels",
                        frame.channels
                    ));
                }

                // Accept waveform
                unsafe {
                    ffi::SherpaOnnxVoiceActivityDetectorAcceptWaveform(
                        self.detector.detector,
                        frame.samples.as_ptr(),
                        frame.samples.len() as i32,
                    );
                }

                // Calculate frame duration and advance time
                let frame_duration_ms =
                    (frame.samples.len() as f64 / frame.sample_rate as f64 * 1000.0) as u64;
                self.absolute_time_ms += frame_duration_ms;

                // Check detection status
                let detected =
                    unsafe { ffi::SherpaOnnxVoiceActivityDetectorDetected(self.detector.detector) };

                // Handle state transitions for Events mode
                if self.config.output_mode == VadOutputMode::Events {
                    let new_state =
                        if detected == 1 { SpeechState::Speaking } else { SpeechState::Idle };

                    if new_state != self.speech_state {
                        match new_state {
                            SpeechState::Speaking => {
                                // Speech started
                                self.speech_start_ms = Some(self.absolute_time_ms);
                                self.emit_event(
                                    "speech_start",
                                    self.absolute_time_ms,
                                    None,
                                    output,
                                )?;
                            },
                            SpeechState::Idle => {
                                // Speech ended
                                if let Some(start_ms) = self.speech_start_ms {
                                    let duration = self.absolute_time_ms.saturating_sub(start_ms);
                                    self.emit_event(
                                        "speech_end",
                                        self.absolute_time_ms,
                                        Some(duration),
                                        output,
                                    )?;
                                }
                                self.speech_start_ms = None;
                            },
                        }
                        self.speech_state = new_state;
                    }
                }

                // Process any complete speech segments
                self.process_segments(output)?;

                Ok(())
            },
            _ => Err("VAD only accepts audio packets".to_string()),
        }
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        let new_config: VadConfig = if let Some(params) = params {
            serde_json::from_value(params).map_err(|e| format!("Invalid configuration: {}", e))?
        } else {
            VadConfig::default()
        };

        plugin_info!(self.logger, "Updating VAD parameters");

        // Only update mutable parameters (not model-affecting ones)
        // Model-affecting params (model_path, num_threads, provider) require re-initialization
        if new_config.model_path != self.config.model_path
            || new_config.num_threads != self.config.num_threads
            || new_config.provider != self.config.provider
        {
            return Err("Cannot change model_path, num_threads, or provider at runtime. \
                 Please destroy and recreate the node."
                .to_string());
        }

        self.config = new_config;
        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_debug!(self.logger, "Flushing VAD detector");

        unsafe {
            ffi::SherpaOnnxVoiceActivityDetectorFlush(self.detector.detector);
        }

        // Process any remaining segments
        self.process_segments(output)?;

        Ok(())
    }

    fn cleanup(&mut self) {
        plugin_info!(self.logger, "Cleaning up VAD plugin");

        // Reset detector state
        unsafe {
            ffi::SherpaOnnxVoiceActivityDetectorReset(self.detector.detector);
        }

        // Detector will be destroyed when Arc is dropped
    }
}
