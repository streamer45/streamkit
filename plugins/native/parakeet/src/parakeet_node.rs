// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::resource_cache::ResourceCache;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    AudioFormat, SampleFormat, TranscriptionData, TranscriptionSegment,
};

use crate::config::ParakeetConfig;
use crate::ffi;
use crate::vad::SileroVAD;

/// Wrapper for recognizer pointer with proper cleanup
struct RecognizerWrapper {
    recognizer: *mut ffi::SherpaOnnxOfflineRecognizer,
}

impl RecognizerWrapper {
    const fn new(recognizer: *mut ffi::SherpaOnnxOfflineRecognizer) -> Self {
        Self { recognizer }
    }

    const fn get(&self) -> *mut ffi::SherpaOnnxOfflineRecognizer {
        self.recognizer
    }
}

unsafe impl Send for RecognizerWrapper {}
unsafe impl Sync for RecognizerWrapper {}

impl Drop for RecognizerWrapper {
    fn drop(&mut self) {
        if !self.recognizer.is_null() {
            unsafe {
                ffi::SherpaOnnxDestroyOfflineRecognizer(self.recognizer);
            }
        }
    }
}

static RECOGNIZER_CACHE: ResourceCache<(String, i32, String), RecognizerWrapper> =
    ResourceCache::new();

pub struct ParakeetNode {
    config: ParakeetConfig,
    recognizer: Arc<RecognizerWrapper>,
    vad: Option<SileroVAD>,

    // Frame buffering (for VAD)
    frame_buffer: VecDeque<f32>,

    // Speech segment buffering (for recognition)
    speech_buffer: VecDeque<f32>,
    segment_start_time_ms: u64,

    // Silence tracking
    silence_frame_count: usize,
    silence_threshold_frames: usize,

    // Time tracking
    absolute_time_ms: u64,

    logger: Logger,
}

// All fields are Send+Sync: Logger and RecognizerWrapper have their
// own manual impls in their respective modules; the remaining fields
// are standard library types that auto-derive Send+Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    // Compilation fails here if any field stops being Send+Sync.
    assert_send_sync::<ParakeetNode>();
};

impl NativeProcessorNode for ParakeetNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("parakeet")
            .description(
                "Fast speech-to-text transcription using NVIDIA Parakeet TDT, a transducer-based \
                 ASR model. Approximately 10x faster than Whisper on consumer hardware with \
                 competitive accuracy. Uses sherpa-onnx for inference. \
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
                        "description": "Path to Parakeet TDT model directory (contains encoder.int8.onnx, decoder.int8.onnx, joiner.int8.onnx, tokens.txt). IMPORTANT: Input audio must be 16kHz mono f32.",
                        "default": "models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
                    },
                    "num_threads": {
                        "type": "integer",
                        "description": "Number of threads for inference",
                        "default": 4,
                        "minimum": 1,
                        "maximum": 16
                    },
                    "execution_provider": {
                        "type": "string",
                        "description": "Execution provider (cpu, cuda, tensorrt)",
                        "default": "cpu",
                        "enum": ["cpu", "cuda", "tensorrt"]
                    },
                    "use_vad": {
                        "type": "boolean",
                        "description": "Enable VAD-based segmentation",
                        "default": true
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
                    "min_speech_duration_ms": {
                        "type": "integer",
                        "description": "Minimum speech duration to transcribe (milliseconds). Shorter segments are discarded as noise.",
                        "default": 300,
                        "minimum": 0,
                        "maximum": 5000
                    }
                }
            }))
            .category("ml")
            .category("speech")
            .category("transcription")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "ParakeetNode::new() called");

        let config: ParakeetConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?
        } else {
            ParakeetConfig::default()
        };

        plugin_info!(
            logger,
            "Config: model_dir={}, num_threads={}, use_vad={}",
            config.model_dir,
            config.num_threads,
            config.use_vad
        );

        // Build paths
        let model_dir = PathBuf::from(&config.model_dir);
        let model_dir = if model_dir.is_absolute() {
            model_dir
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Failed to get current dir: {e}"))?
                .join(model_dir)
        };

        // Canonicalize
        let model_dir = model_dir.canonicalize().map_err(|e| {
            format!("Failed to canonicalize model dir '{}': {}", model_dir.display(), e)
        })?;

        let model_dir_str = model_dir.to_string_lossy().to_string();

        // Cache key: (model_dir, num_threads, execution_provider)
        let cache_key = (model_dir_str, config.num_threads, config.execution_provider.clone());

        plugin_info!(
            logger,
            "Cache key: dir='{}' threads={} provider='{}'",
            cache_key.0,
            cache_key.1,
            cache_key.2
        );

        let recognizer = RECOGNIZER_CACHE.get_or_init(cache_key, |_| {
            plugin_info!(logger, "CACHE MISS: Creating new recognizer");
            let recognizer_ptr = unsafe { create_recognizer(&logger, &model_dir, &config)? };
            Ok(RecognizerWrapper::new(recognizer_ptr))
        })
        .map_err(|e| format!("recognizer cache: {e}"))?;

        // Initialize VAD if enabled
        let vad = if config.use_vad {
            plugin_info!(logger, "Initializing Silero VAD");
            let vad_instance = SileroVAD::new(&config.vad_model_path, 16000, config.vad_threshold)
                .map_err(|e| format!("Failed to initialize VAD: {e}"))?;
            Some(vad_instance)
        } else {
            plugin_info!(logger, "VAD disabled");
            None
        };

        // Calculate silence threshold in frames (each frame is 32ms)
        let silence_threshold_frames = (config.min_silence_duration_ms / 32) as usize;

        Ok(Self {
            config,
            recognizer,
            vad,
            frame_buffer: VecDeque::with_capacity(1024),
            speech_buffer: VecDeque::with_capacity(16000 * 30), // 30 seconds max
            segment_start_time_ms: 0,
            silence_frame_count: 0,
            silence_threshold_frames,
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
                        "Parakeet requires 16kHz audio, got {}Hz. Add audio_resampler upstream.",
                        frame.sample_rate
                    ));
                }
                if frame.channels != 1 {
                    return Err(format!(
                        "Parakeet requires mono audio, got {} channels. Add audio_resampler upstream.",
                        frame.channels
                    ));
                }

                let has_vad = self.vad.is_some();

                if has_vad {
                    // VAD-based processing
                    self.frame_buffer.extend(frame.samples.as_ref().as_slice().iter().copied());

                    // Process complete 512-sample frames through VAD
                    loop {
                        if self.frame_buffer.len() < 512 {
                            break;
                        }

                        let vad_frame: Vec<f32> = self.frame_buffer.drain(..512).collect();

                        // Process chunk through VAD - scope the borrow
                        let is_speech = {
                            let vad = self.vad.as_mut().ok_or_else(|| {
                                "VAD not initialized but use_vad is true".to_string()
                            })?;
                            let probability = vad
                                .process_chunk(&vad_frame)
                                .map_err(|e| format!("VAD processing failed: {e}"))?;
                            probability >= self.config.vad_threshold
                        }; // vad borrow ends here

                        let should_transcribe = if is_speech {
                            // Speech detected
                            self.silence_frame_count = 0;

                            // Start new segment if needed
                            if self.speech_buffer.is_empty() {
                                self.segment_start_time_ms = self.absolute_time_ms;
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
                            segment_duration_ms >= max_duration_ms
                        } else {
                            // Silence detected
                            self.silence_frame_count += 1;

                            // Check if we have buffered speech and enough silence
                            !self.speech_buffer.is_empty()
                                && self.silence_frame_count >= self.silence_threshold_frames
                        };

                        self.absolute_time_ms += 32; // 512 samples @ 16kHz = 32ms

                        // Transcribe after releasing the borrow on vad
                        if should_transcribe {
                            self.transcribe_and_emit(output)?;
                        }
                    }
                } else {
                    // No VAD: accumulate samples and transcribe when reaching max duration
                    if self.speech_buffer.is_empty() {
                        self.segment_start_time_ms = self.absolute_time_ms;
                    }

                    self.speech_buffer.extend(frame.samples.as_ref().as_slice().iter().copied());

                    // Allow: Sample count and rate are always positive, cast is safe
                    #[allow(
                        clippy::cast_precision_loss,
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss
                    )]
                    let duration_ms = (frame.samples.len() as f32 / 16.0) as u64;
                    self.absolute_time_ms += duration_ms;

                    let segment_duration_ms = self.absolute_time_ms - self.segment_start_time_ms;
                    // Allow: Config value is always positive, cast to u64 for duration comparison
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let max_duration_ms = (self.config.max_segment_duration_secs * 1000.0) as u64;

                    if segment_duration_ms >= max_duration_ms {
                        self.transcribe_and_emit(output)?;
                    }
                }

                Ok(())
            },
            _ => Err("Parakeet plugin only accepts audio packets".to_string()),
        }
    }

    fn update_params(&mut self, _params: Option<serde_json::Value>) -> Result<(), String> {
        // Runtime parameter updates not yet implemented.
        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_info!(self.logger, "Flush called, buffer_len={}", self.speech_buffer.len());

        if !self.speech_buffer.is_empty() {
            self.transcribe_and_emit(output)?;
        }

        Ok(())
    }

    fn cleanup(&mut self) {
        if !self.speech_buffer.is_empty() {
            plugin_warn!(
                self.logger,
                "Speech buffer not empty at cleanup: {} samples",
                self.speech_buffer.len()
            );
        }
    }
}

impl ParakeetNode {
    /// Transcribe buffered speech segment and emit result
    fn transcribe_and_emit(&mut self, output: &OutputSender) -> Result<(), String> {
        if self.speech_buffer.is_empty() {
            return Ok(());
        }

        let samples: Vec<f32> = self.speech_buffer.drain(..).collect();

        // Allow: Sample count / sample rate for duration calculation
        #[allow(clippy::cast_precision_loss)]
        let duration_secs = samples.len() as f32 / 16000.0;

        // Skip segments shorter than the minimum speech duration —
        // these are typically noise bursts that produce hallucinated
        // output from the recognizer.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let duration_ms = (duration_secs * 1000.0) as u64;
        if duration_ms < self.config.min_speech_duration_ms {
            plugin_info!(
                self.logger,
                "Skipping short segment: {} samples ({:.2}s < {}ms minimum)",
                samples.len(),
                duration_secs,
                self.config.min_speech_duration_ms
            );
            self.silence_frame_count = 0;
            return Ok(());
        }

        plugin_info!(
            self.logger,
            "Transcribing segment: {} samples ({:.2}s)",
            samples.len(),
            duration_secs
        );

        // Create stream
        let stream = unsafe { ffi::SherpaOnnxCreateOfflineStream(self.recognizer.get()) };

        if stream.is_null() {
            return Err("Failed to create recognition stream".to_string());
        }

        // Accept waveform
        // Allow: Sample count is guaranteed to fit in i32 for practical audio segments
        #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
        unsafe {
            ffi::SherpaOnnxAcceptWaveformOffline(
                stream,
                16000,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }

        // Decode
        unsafe {
            ffi::SherpaOnnxDecodeOfflineStream(self.recognizer.get(), stream);
        }

        // Get result
        let result_ptr = unsafe { ffi::SherpaOnnxGetOfflineStreamResult(stream) };

        if result_ptr.is_null() {
            unsafe {
                ffi::SherpaOnnxDestroyOfflineStream(stream);
            }
            return Err("Recognition returned null result".to_string());
        }

        let result = unsafe { &*result_ptr };

        // Extract text
        let text = if result.text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(result.text).to_string_lossy().into_owned() }
        };

        // Extract detected language from the FFI result (v3 model supports 25 languages).
        let language = if result.lang.is_null() {
            None
        } else {
            let lang = unsafe { CStr::from_ptr(result.lang).to_string_lossy().into_owned() };
            if lang.is_empty() {
                None
            } else {
                Some(lang)
            }
        };

        // Cleanup
        unsafe {
            ffi::SherpaOnnxDestroyOfflineRecognizerResult(result_ptr);
            ffi::SherpaOnnxDestroyOfflineStream(stream);
        }

        // Emit transcription if not empty
        if !text.trim().is_empty() {
            plugin_info!(self.logger, "Transcription: {}", text);

            let segment = TranscriptionSegment {
                text: text.trim().to_string(),
                start_time_ms: self.segment_start_time_ms,
                end_time_ms: self.absolute_time_ms,
                confidence: None,
            };

            output.send(
                "out",
                &Packet::Transcription(std::sync::Arc::new(TranscriptionData {
                    text: segment.text.clone(),
                    segments: vec![segment],
                    language,
                    metadata: None,
                })),
            )?;
        }

        // Reset for next segment
        self.silence_frame_count = 0;

        Ok(())
    }
}

/// Create recognizer using sherpa-onnx C API with transducer (encoder/decoder/joiner) config
unsafe fn create_recognizer(
    logger: &Logger,
    model_dir: &Path,
    config: &ParakeetConfig,
) -> Result<*mut ffi::SherpaOnnxOfflineRecognizer, String> {
    plugin_info!(logger, "Creating recognizer for model_dir={}", model_dir.display());

    let encoder_path = model_dir.join("encoder.int8.onnx");
    let decoder_path = model_dir.join("decoder.int8.onnx");
    let joiner_path = model_dir.join("joiner.int8.onnx");
    let tokens_path = model_dir.join("tokens.txt");

    // Verify files exist
    for (name, path) in [
        ("encoder", &encoder_path),
        ("decoder", &decoder_path),
        ("joiner", &joiner_path),
        ("tokens", &tokens_path),
    ] {
        if !path.exists() {
            return Err(format!("{} file not found: {}", name, path.display()));
        }
        plugin_info!(logger, "File exists: {}", name);
    }

    // Create C strings - keep all alive until after the FFI call
    let encoder_cstr = path_to_cstring(&encoder_path)?;
    let decoder_cstr = path_to_cstring(&decoder_path)?;
    let joiner_cstr = path_to_cstring(&joiner_path)?;
    let tokens_cstr = path_to_cstring(&tokens_path)?;

    plugin_info!(
        logger,
        "Initializing recognizer with execution_provider='{}'",
        config.execution_provider
    );
    let provider_cstr = CString::new(config.execution_provider.as_str())
        .map_err(|_| "Invalid execution provider string".to_string())?;
    let decoding_method_cstr =
        CString::new("greedy_search").map_err(|_| "Invalid decoding method string".to_string())?;

    // Empty strings for unused fields (safer than null)
    let empty_cstr = CString::new("").map_err(|_| "Invalid empty string".to_string())?;

    // Build config struct — populate transducer fields, leave all others empty
    let recognizer_config = ffi::SherpaOnnxOfflineRecognizerConfig {
        feat_config: ffi::SherpaOnnxFeatureConfig { sample_rate: 16000, feature_dim: 80 },
        model_config: ffi::SherpaOnnxOfflineModelConfig {
            transducer: ffi::SherpaOnnxOfflineTransducerModelConfig {
                encoder: encoder_cstr.as_ptr(),
                decoder: decoder_cstr.as_ptr(),
                joiner: joiner_cstr.as_ptr(),
            },
            paraformer: ffi::SherpaOnnxOfflineParaformerModelConfig { model: empty_cstr.as_ptr() },
            nemo_ctc: ffi::SherpaOnnxOfflineNemoEncDecCtcModelConfig { model: empty_cstr.as_ptr() },
            whisper: ffi::SherpaOnnxOfflineWhisperModelConfig {
                encoder: empty_cstr.as_ptr(),
                decoder: empty_cstr.as_ptr(),
                language: empty_cstr.as_ptr(),
                task: empty_cstr.as_ptr(),
                tail_paddings: 0,
            },
            tdnn: ffi::SherpaOnnxOfflineTdnnModelConfig { model: empty_cstr.as_ptr() },
            tokens: tokens_cstr.as_ptr(),
            num_threads: config.num_threads,
            debug: 0,
            provider: provider_cstr.as_ptr(),
            model_type: empty_cstr.as_ptr(),
            modeling_unit: empty_cstr.as_ptr(),
            bpe_vocab: empty_cstr.as_ptr(),
            telespeech_ctc: empty_cstr.as_ptr(),
            sense_voice: ffi::SherpaOnnxOfflineSenseVoiceModelConfig {
                model: empty_cstr.as_ptr(),
                language: empty_cstr.as_ptr(),
                use_itn: 0,
            },
            moonshine: ffi::SherpaOnnxOfflineMoonshineModelConfig {
                preprocessor: empty_cstr.as_ptr(),
                encoder: empty_cstr.as_ptr(),
                uncached_decoder: empty_cstr.as_ptr(),
                cached_decoder: empty_cstr.as_ptr(),
            },
            fire_red_asr: ffi::SherpaOnnxOfflineFireRedAsrModelConfig {
                encoder: empty_cstr.as_ptr(),
                decoder: empty_cstr.as_ptr(),
            },
            dolphin: ffi::SherpaOnnxOfflineDolphinModelConfig { model: empty_cstr.as_ptr() },
            zipformer_ctc: ffi::SherpaOnnxOfflineZipformerCtcModelConfig {
                model: empty_cstr.as_ptr(),
            },
            canary: ffi::SherpaOnnxOfflineCanaryModelConfig {
                encoder: empty_cstr.as_ptr(),
                decoder: empty_cstr.as_ptr(),
                src_lang: empty_cstr.as_ptr(),
                tgt_lang: empty_cstr.as_ptr(),
                use_pnc: 0,
            },
            wenet_ctc: ffi::SherpaOnnxOfflineWenetCtcModelConfig { model: empty_cstr.as_ptr() },
            omnilingual: ffi::SherpaOnnxOfflineOmnilingualAsrCtcModelConfig {
                model: empty_cstr.as_ptr(),
            },
        },
        lm_config: ffi::SherpaOnnxOfflineLMConfig { model: empty_cstr.as_ptr(), scale: 0.0 },
        decoding_method: decoding_method_cstr.as_ptr(),
        max_active_paths: 4,
        hotwords_file: empty_cstr.as_ptr(),
        hotwords_score: 0.0,
        rule_fsts: empty_cstr.as_ptr(),
        rule_fars: empty_cstr.as_ptr(),
        blank_penalty: 0.0,
        hr: ffi::SherpaOnnxHomophoneReplacerConfig {
            dict_dir: empty_cstr.as_ptr(),
            lexicon: empty_cstr.as_ptr(),
            rule_fsts: empty_cstr.as_ptr(),
        },
    };

    plugin_info!(logger, "Calling SherpaOnnxCreateOfflineRecognizer");
    let recognizer = ffi::SherpaOnnxCreateOfflineRecognizer(&raw const recognizer_config);

    if recognizer.is_null() {
        return Err("Failed to create recognizer".to_string());
    }

    plugin_info!(logger, "Recognizer created successfully");
    Ok(recognizer)
}

fn path_to_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| format!("Invalid path: {e}"))
}
