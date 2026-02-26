// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::sync::Mutex;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    AudioFormat, SampleFormat, TranscriptionData, TranscriptionSegment,
};

use crate::config::MoonshineConfig;
use crate::ffi;

/// Wrapper for a Moonshine transcriber handle with proper cleanup.
struct TranscriberHandle {
    handle: i32,
}

impl TranscriberHandle {
    const fn new(handle: i32) -> Self {
        Self { handle }
    }

    const fn get(&self) -> i32 {
        self.handle
    }
}

// SAFETY: The Moonshine C API is thread-safe. All API calls are thread-safe and
// calculations on a single transcriber are serialized internally.
unsafe impl Send for TranscriberHandle {}
unsafe impl Sync for TranscriberHandle {}

impl Drop for TranscriberHandle {
    fn drop(&mut self) {
        if self.handle >= 0 {
            unsafe {
                ffi::moonshine_free_transcriber(self.handle);
            }
        }
    }
}

/// Global cache of transcriber handles.
/// Key: (model_dir, model_arch)
// Allow: Type complexity is acceptable here - composite key for caching transcribers
#[allow(clippy::type_complexity)]
static TRANSCRIBER_CACHE: std::sync::LazyLock<
    Mutex<HashMap<(String, String), std::sync::Arc<TranscriberHandle>>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct MoonshineNode {
    config: MoonshineConfig,
    transcriber: std::sync::Arc<TranscriberHandle>,
    /// Stream handle for the Moonshine streaming API.
    /// Each node has its own stream, while the transcriber may be shared.
    stream_handle: i32,
    /// Number of completed lines seen so far in streaming mode.
    /// Used to avoid re-emitting already-emitted completed lines.
    emitted_line_count: u64,
    /// Track the last incomplete line ID to detect text changes.
    last_partial_line_id: Option<u64>,
    /// Time tracking for absolute position in the audio stream.
    absolute_time_ms: u64,
    logger: Logger,
}

impl NativeProcessorNode for MoonshineNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("moonshine")
            .description(
                "Speech-to-text transcription using the Moonshine model family. \
                 Supports both streaming (partial results while speaking) and non-streaming modes \
                 with built-in VAD. No external VAD model required. \
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
                    "model_dir": {
                        "type": "string",
                        "description": "Path to Moonshine model directory (containing encoder_model.ort, decoder_model_merged.ort, tokenizer.bin). IMPORTANT: Input audio must be 16kHz mono f32.",
                        "default": "models/moonshine-base-en"
                    },
                    "model_arch": {
                        "type": "string",
                        "description": "Model architecture (tiny, base, tiny_streaming, base_streaming, small_streaming, medium_streaming). Streaming models emit partial results while the user is still speaking.",
                        "default": "base",
                        "enum": ["tiny", "base", "tiny_streaming", "base_streaming", "small_streaming", "medium_streaming"]
                    },
                }
            }))
            .category("ml")
            .category("speech")
            .category("transcription")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "MoonshineNode::new() called");

        let config: MoonshineConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?
        } else {
            MoonshineConfig::default()
        };

        plugin_info!(
            logger,
            "Config: model_dir={}, model_arch={}",
            config.model_dir,
            config.model_arch
        );

        // Build absolute model path
        let model_dir = PathBuf::from(&config.model_dir);
        let model_dir = if model_dir.is_absolute() {
            model_dir
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Failed to get current dir: {e}"))?
                .join(model_dir)
        };

        let model_dir = model_dir.canonicalize().map_err(|e| {
            format!("Failed to canonicalize model dir '{}': {}", model_dir.display(), e)
        })?;

        let model_dir_str = model_dir.to_string_lossy().to_string();

        // Cache key: (model_dir, model_arch)
        let cache_key = (model_dir_str.clone(), config.model_arch.clone());

        plugin_info!(logger, "Cache key: dir='{}' arch='{}'", cache_key.0, cache_key.1);

        // Check cache
        let cached_handle = {
            let cache = TRANSCRIBER_CACHE
                .lock()
                .map_err(|e| format!("Failed to lock transcriber cache: {e}"))?;

            plugin_info!(logger, "Cache has {} entries", cache.len());
            cache.get(&cache_key).cloned()
        };

        let transcriber = if let Some(handle) = cached_handle {
            plugin_info!(logger, "CACHE HIT: Reusing cached transcriber");
            handle
        } else {
            plugin_info!(logger, "CACHE MISS: Creating new transcriber");

            let arch = config.arch_to_ffi()?;

            let model_path_cstr = CString::new(model_dir_str.as_bytes())
                .map_err(|e| format!("Invalid model dir path: {e}"))?;

            plugin_info!(
                logger,
                "Loading transcriber from '{}' with arch={}",
                model_dir_str,
                config.model_arch
            );

            let handle = unsafe {
                ffi::moonshine_load_transcriber_from_files(
                    model_path_cstr.as_ptr(),
                    arch,
                    std::ptr::null(),
                    0,
                    ffi::MOONSHINE_HEADER_VERSION,
                )
            };

            if handle < 0 {
                let error_msg = unsafe {
                    let ptr = ffi::moonshine_error_to_string(handle);
                    if ptr.is_null() {
                        format!("Unknown error (code {handle})")
                    } else {
                        CStr::from_ptr(ptr).to_string_lossy().into_owned()
                    }
                };
                return Err(format!("Failed to load Moonshine transcriber: {error_msg}"));
            }

            let handle_arc = std::sync::Arc::new(TranscriberHandle::new(handle));

            // Insert into cache
            let cache_size = {
                let mut cache = TRANSCRIBER_CACHE
                    .lock()
                    .map_err(|e| format!("Failed to lock transcriber cache: {e}"))?;

                cache.insert(cache_key, handle_arc.clone());
                cache.len()
            };
            plugin_info!(logger, "Cache now has {} entries", cache_size);

            handle_arc
        };

        // Create and start a stream for this node instance.
        // Each node gets its own stream, while the transcriber may be shared.
        let stream_handle = unsafe { ffi::moonshine_create_stream(transcriber.get(), 0) };
        if stream_handle < 0 {
            let error_msg = unsafe {
                let ptr = ffi::moonshine_error_to_string(stream_handle);
                if ptr.is_null() {
                    format!("Unknown error (code {stream_handle})")
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(format!("Failed to create Moonshine stream: {error_msg}"));
        }

        let start_err = unsafe { ffi::moonshine_start_stream(transcriber.get(), stream_handle) };
        if start_err != ffi::MOONSHINE_ERROR_NONE {
            let error_msg = unsafe {
                let ptr = ffi::moonshine_error_to_string(start_err);
                if ptr.is_null() {
                    format!("Unknown error (code {start_err})")
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(format!("Failed to start Moonshine stream: {error_msg}"));
        }

        plugin_info!(logger, "Stream created and started (handle={})", stream_handle);

        Ok(Self {
            config,
            transcriber,
            stream_handle,
            emitted_line_count: 0,
            last_partial_line_id: None,
            absolute_time_ms: 0,
            logger,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Audio(frame) => {
                // Validate audio format (must be 16kHz mono f32)
                if frame.sample_rate != 16000 {
                    return Err(format!(
                        "Moonshine requires 16kHz audio, got {}Hz. Add audio::resampler upstream.",
                        frame.sample_rate
                    ));
                }
                if frame.channels != 1 {
                    return Err(format!(
                        "Moonshine requires mono audio, got {} channels. Add audio::resampler upstream.",
                        frame.channels
                    ));
                }

                let samples = frame.samples.as_ref().as_slice();

                // Track time
                // Allow: Sample count / sample rate for duration calculation
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss
                )]
                let duration_ms = (samples.len() as f64 / 16.0) as u64;

                // Step 1: Add audio data to the stream buffer.
                // This is a lightweight operation that does no processing.
                let add_err = unsafe {
                    ffi::moonshine_transcribe_add_audio_to_stream(
                        self.transcriber.get(),
                        self.stream_handle,
                        samples.as_ptr(),
                        samples.len() as u64,
                        16000,
                        0, // no flags
                    )
                };

                if add_err != ffi::MOONSHINE_ERROR_NONE {
                    let error_msg = unsafe {
                        let ptr = ffi::moonshine_error_to_string(add_err);
                        if ptr.is_null() {
                            format!("Unknown error (code {add_err})")
                        } else {
                            CStr::from_ptr(ptr).to_string_lossy().into_owned()
                        }
                    };
                    return Err(format!("Moonshine add audio error: {error_msg}"));
                }

                // Step 2: Request an updated transcript.
                // The library internally throttles to ~200ms between full analyses.
                let mut transcript_ptr: *mut ffi::Transcript = std::ptr::null_mut();
                let transcribe_err = unsafe {
                    ffi::moonshine_transcribe_stream(
                        self.transcriber.get(),
                        self.stream_handle,
                        0, // no flags — let the library throttle
                        &raw mut transcript_ptr,
                    )
                };

                if transcribe_err != ffi::MOONSHINE_ERROR_NONE {
                    let error_msg = unsafe {
                        let ptr = ffi::moonshine_error_to_string(transcribe_err);
                        if ptr.is_null() {
                            format!("Unknown error (code {transcribe_err})")
                        } else {
                            CStr::from_ptr(ptr).to_string_lossy().into_owned()
                        }
                    };
                    return Err(format!("Moonshine transcription error: {error_msg}"));
                }

                if !transcript_ptr.is_null() {
                    self.process_transcript(transcript_ptr, output)?;
                }

                self.absolute_time_ms += duration_ms;

                Ok(())
            },
            _ => Err("Moonshine plugin only accepts audio packets".to_string()),
        }
    }

    fn update_params(&mut self, _params: Option<serde_json::Value>) -> Result<(), String> {
        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_info!(self.logger, "Flush called, finalizing stream");

        // Stop the stream to signal no more audio is coming.
        let stop_err =
            unsafe { ffi::moonshine_stop_stream(self.transcriber.get(), self.stream_handle) };
        if stop_err != ffi::MOONSHINE_ERROR_NONE {
            let error_msg = unsafe {
                let ptr = ffi::moonshine_error_to_string(stop_err);
                if ptr.is_null() {
                    format!("Unknown error (code {stop_err})")
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(format!("Moonshine stop stream error: {error_msg}"));
        }

        // Do a final transcription with FORCE_UPDATE to ensure all audio is processed.
        let mut transcript_ptr: *mut ffi::Transcript = std::ptr::null_mut();
        let transcribe_err = unsafe {
            ffi::moonshine_transcribe_stream(
                self.transcriber.get(),
                self.stream_handle,
                ffi::MOONSHINE_FLAG_FORCE_UPDATE,
                &raw mut transcript_ptr,
            )
        };

        if transcribe_err != ffi::MOONSHINE_ERROR_NONE {
            let error_msg = unsafe {
                let ptr = ffi::moonshine_error_to_string(transcribe_err);
                if ptr.is_null() {
                    format!("Unknown error (code {transcribe_err})")
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(format!("Moonshine flush transcription error: {error_msg}"));
        }

        if !transcript_ptr.is_null() {
            self.process_transcript(transcript_ptr, output)?;
        }

        Ok(())
    }

    fn cleanup(&mut self) {
        plugin_info!(self.logger, "Cleanup called, freeing stream");
        if self.stream_handle >= 0 {
            unsafe {
                ffi::moonshine_free_stream(self.transcriber.get(), self.stream_handle);
            }
            self.stream_handle = -1;
        }
    }
}

impl MoonshineNode {
    /// Process a transcript returned by the Moonshine C API and emit
    /// completed transcription lines downstream.
    ///
    /// For streaming models, we track which lines have already been emitted
    /// (by `emitted_line_count`) and only emit newly completed lines.
    /// Partial (incomplete) lines are not emitted to avoid duplicate output.
    fn process_transcript(
        &mut self,
        transcript_ptr: *mut ffi::Transcript,
        output: &OutputSender,
    ) -> Result<(), String> {
        let transcript = unsafe { &*transcript_ptr };
        let line_count = transcript.line_count;

        if line_count == 0 {
            return Ok(());
        }

        // Allow: line_count from FFI is bounded by available memory; truncation won't occur
        #[allow(clippy::cast_possible_truncation)]
        let lines = unsafe { std::slice::from_raw_parts(transcript.lines, line_count as usize) };

        if self.config.is_streaming() {
            self.process_streaming_lines(lines, output)?;
        } else {
            self.process_oneshot_lines(lines, output)?;
        }

        Ok(())
    }

    /// Process lines from a streaming transcript.
    ///
    /// Only emit lines that are newly completed (is_complete != 0) and haven't
    /// been emitted before. Track partial lines for potential future emission.
    fn process_streaming_lines(
        &mut self,
        lines: &[ffi::TranscriptLine],
        output: &OutputSender,
    ) -> Result<(), String> {
        for (i, line) in lines.iter().enumerate() {
            // Allow: index is bounded by line_count which fits in u64
            #[allow(clippy::cast_possible_truncation)]
            let line_idx = i as u64;

            // Skip lines we've already emitted
            if line_idx < self.emitted_line_count {
                continue;
            }

            let text = if line.text.is_null() {
                continue;
            } else {
                unsafe { CStr::from_ptr(line.text).to_string_lossy() }
            };

            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }

            if line.is_complete != 0 {
                // This line is finalized — emit it
                // Allow: start_time is in seconds, converting to ms
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let start_time_ms = (line.start_time * 1000.0) as u64;
                // Allow: duration is in seconds, converting to ms
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let end_time_ms = start_time_ms + (line.duration * 1000.0) as u64;

                let segment = TranscriptionSegment {
                    text: trimmed.to_string(),
                    start_time_ms,
                    end_time_ms,
                    confidence: None,
                };

                plugin_info!(self.logger, "Emitting completed line {}: '{}'", line_idx, trimmed);

                output.send(
                    "out",
                    &Packet::Transcription(std::sync::Arc::new(TranscriptionData {
                        text: segment.text.clone(),
                        segments: vec![segment],
                        language: Some("en".to_string()),
                        metadata: None,
                    })),
                )?;

                self.emitted_line_count = line_idx + 1;
                self.last_partial_line_id = None;
            } else {
                // Incomplete line — track it but don't emit
                self.last_partial_line_id = Some(line.id);
            }
        }

        Ok(())
    }

    /// Process lines from a non-streaming (oneshot) transcript.
    ///
    /// The Moonshine API returns the full accumulated transcript on every call,
    /// so we track `emitted_line_count` to only emit newly completed lines.
    fn process_oneshot_lines(
        &mut self,
        lines: &[ffi::TranscriptLine],
        output: &OutputSender,
    ) -> Result<(), String> {
        for (i, line) in lines.iter().enumerate() {
            // Allow: index is bounded by line_count which fits in u64
            #[allow(clippy::cast_possible_truncation)]
            let line_idx = i as u64;

            // Skip lines we've already emitted
            if line_idx < self.emitted_line_count {
                continue;
            }

            let text = if line.text.is_null() {
                continue;
            } else {
                unsafe { CStr::from_ptr(line.text).to_string_lossy() }
            };

            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Allow: start_time is in seconds, converting to ms
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let start_time_ms = (line.start_time * 1000.0) as u64;
            // Allow: duration is in seconds, converting to ms
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let end_time_ms = start_time_ms + (line.duration * 1000.0) as u64;

            let segment = TranscriptionSegment {
                text: trimmed.to_string(),
                start_time_ms,
                end_time_ms,
                confidence: None,
            };

            plugin_info!(self.logger, "Transcription: '{}'", trimmed);

            output.send(
                "out",
                &Packet::Transcription(std::sync::Arc::new(TranscriptionData {
                    text: segment.text.clone(),
                    segments: vec![segment],
                    language: Some("en".to_string()),
                    metadata: None,
                })),
            )?;

            self.emitted_line_count = line_idx + 1;
        }

        Ok(())
    }
}
