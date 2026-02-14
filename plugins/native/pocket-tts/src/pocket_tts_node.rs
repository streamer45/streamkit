// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::borrow::Cow;
use std::sync::Arc;

use pocket_tts::ModelState;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{AudioFormat, SampleFormat};
use streamkit_plugin_sdk_native::{plugin_debug, plugin_error, plugin_info, plugin_warn};

use crate::config::PocketTtsConfig;
use crate::model::{configure_model, get_or_load_model, ModelCacheKey};
use crate::sentence_splitter::SentenceSplitter;
use crate::voice::{
    get_or_load_voice_state, normalize_voice_spec, voice_state_from_base64,
    voice_state_from_wav_bytes, VoiceCacheKey,
};

pub struct PocketTtsNode {
    model: pocket_tts::TTSModel,
    model_key: ModelCacheKey,
    voice_state: Arc<ModelState>,
    voice_buffer: Vec<u8>,
    voice_expected_len: Option<usize>,
    voice_input_seen: bool,
    voice_ready: bool,
    config: PocketTtsConfig,
    text_buffer: String,
    sentence_splitter: SentenceSplitter,
    logger: Logger,
}

impl NativeProcessorNode for PocketTtsNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("pocket-tts")
            .description(
                "Lightweight CPU TTS using Kyutai Pocket TTS (Candle). \
                 English-only voices with streaming output. \
                 Outputs 24kHz mono audio.",
            )
            .input("in", &[PacketType::Text, PacketType::Binary])
            .input("in_0", &[PacketType::Text, PacketType::Binary])
            .input("in_1", &[PacketType::Binary, PacketType::Text])
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
                    "variant": {
                        "type": "string",
                        "description": "Model variant (config in pocket-tts crate)",
                        "default": "b6369a24"
                    },
                    "config_path": {
                        "type": ["string", "null"],
                        "description": "Optional config YAML path for custom variants/offline use",
                        "default": null
                    },
                    "weights_path": {
                        "type": ["string", "null"],
                        "description": "Local weights path for offline loading",
                        "default": null
                    },
                    "tokenizer_path": {
                        "type": ["string", "null"],
                        "description": "Local tokenizer path for offline loading",
                        "default": null
                    },
                    "voice_embeddings_dir": {
                        "type": ["string", "null"],
                        "description": "Directory with predefined voice embeddings (alba, marius, ...)",
                        "default": null
                    },
                    "voice": {
                        "type": "string",
                        "description": "Voice name, local .wav/.safetensors, hf:// URL, or base64 audio",
                        "default": "alba"
                    },
                    "temperature": {
                        "type": "number",
                        "description": "Sampling temperature (higher = more variation)",
                        "default": 0.7,
                        "minimum": 0.1,
                        "maximum": 2.0
                    },
                    "lsd_decode_steps": {
                        "type": "integer",
                        "description": "LSD decode steps (higher = better quality, slower)",
                        "default": 1,
                        "minimum": 1,
                        "maximum": 8
                    },
                    "eos_threshold": {
                        "type": "number",
                        "description": "End-of-sequence threshold (more negative = longer output)",
                        "default": -4.0,
                        "minimum": -10.0,
                        "maximum": 0.0
                    },
                    "noise_clamp": {
                        "type": ["number", "null"],
                        "description": "Optional noise clamp (null disables)",
                        "default": null
                    },
                    "min_sentence_length": {
                        "type": "integer",
                        "description": "Minimum chars before triggering TTS",
                        "default": 10,
                        "minimum": 1
                    },
                    "quantized": {
                        "type": "boolean",
                        "description": "Enable int8 quantized weights (requires plugin built with feature 'quantized')",
                        "default": false
                    }
                }
            }))
            .category("audio")
            .category("tts")
            .category("ml")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "Pocket TTS plugin new() called with params: {:?}", params);

        let config: PocketTtsConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| {
                let msg = format!("Config parse error: {e}");
                plugin_error!(logger, "{msg}");
                msg
            })?
        } else {
            PocketTtsConfig::default()
        };

        plugin_info!(
            logger,
            "Pocket TTS config: variant={}, quantized={}, offline_weights={}, offline_tokenizer={}, config_path={}, voice_dir={}",
            config.variant,
            config.quantized,
            config.weights_path.is_some(),
            config.tokenizer_path.is_some(),
            config.config_path.as_deref().unwrap_or("none"),
            config.voice_embeddings_dir.as_deref().unwrap_or("none")
        );

        let model_key = ModelCacheKey::from_config(&config);
        let base_model = get_or_load_model(&model_key, &config, &logger).map_err(|e| {
            plugin_error!(logger, "Pocket TTS model load failed: {e}");
            e
        })?;
        let mut model = (*base_model).clone();
        configure_model(&mut model, &config, &logger);

        let voice_dir = config.voice_embeddings_dir.as_deref();
        let voice_spec = normalize_voice_spec(&config.voice, voice_dir);
        let voice_key = VoiceCacheKey { model_key: model_key.clone(), voice_spec };
        let voice_state =
            get_or_load_voice_state(&model, &voice_key, voice_dir, &logger).map_err(|e| {
                plugin_error!(logger, "Pocket TTS voice load failed: {e}");
                e
            })?;

        Ok(Self {
            model,
            model_key,
            voice_state,
            voice_buffer: Vec::new(),
            voice_expected_len: None,
            voice_input_seen: false,
            voice_ready: false,
            config: config.clone(),
            text_buffer: String::new(),
            sentence_splitter: SentenceSplitter::new(config.min_sentence_length),
            logger,
        })
    }

    fn process(&mut self, pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match pin {
            "in" | "in_0" => self.handle_text(&packet, output),
            "in_1" | "voice" => self.handle_voice(packet, output),
            other => Err(format!("Unsupported input pin '{other}'")),
        }
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        if let Some(p) = params {
            let new_config: PocketTtsConfig = serde_json::from_value(p).map_err(|e| {
                let msg = format!("Config parse error: {e}");
                plugin_error!(self.logger, "{msg}");
                msg
            })?;

            let new_model_key = ModelCacheKey::from_config(&new_config);
            let model_changed = new_model_key != self.model_key;
            if model_changed {
                plugin_info!(self.logger, "Model parameters changed, reloading model");
                let base_model = get_or_load_model(&new_model_key, &new_config, &self.logger)
                    .map_err(|e| {
                        plugin_error!(self.logger, "Pocket TTS model load failed: {e}");
                        e
                    })?;
                let mut model = (*base_model).clone();
                configure_model(&mut model, &new_config, &self.logger);
                self.model = model;
                self.model_key = new_model_key.clone();
            } else {
                configure_model(&mut self.model, &new_config, &self.logger);
            }

            let new_voice_dir = new_config.voice_embeddings_dir.as_deref();
            let current_voice_dir = self.config.voice_embeddings_dir.as_deref();
            let new_voice_spec = normalize_voice_spec(&new_config.voice, new_voice_dir);
            let current_voice_spec = normalize_voice_spec(&self.config.voice, current_voice_dir);
            let voice_dir_changed =
                new_config.voice_embeddings_dir != self.config.voice_embeddings_dir;

            if new_voice_spec != current_voice_spec || model_changed || voice_dir_changed {
                let voice_key =
                    VoiceCacheKey { model_key: new_model_key, voice_spec: new_voice_spec };
                self.voice_state =
                    get_or_load_voice_state(&self.model, &voice_key, new_voice_dir, &self.logger)
                        .map_err(|e| {
                        plugin_error!(self.logger, "Pocket TTS voice load failed: {e}");
                        e
                    })?;
                self.voice_buffer.clear();
                self.voice_expected_len = None;
                self.voice_input_seen = false;
                self.voice_ready = false;
            }

            if new_config.min_sentence_length != self.config.min_sentence_length {
                self.sentence_splitter = SentenceSplitter::new(new_config.min_sentence_length);
            }

            self.config = new_config;
        }
        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        if self.voice_input_seen && !self.voice_ready {
            if self.voice_buffer.is_empty() {
                return Err("Voice prompt not received for voice cloning".to_string());
            }
            self.load_voice_from_buffer()?;
        }

        if !self.text_buffer.is_empty() {
            self.flush_text_buffer(output)?;

            if !self.text_buffer.is_empty() {
                let text = self.text_buffer.clone();
                plugin_info!(self.logger, "Flushing remaining text buffer");
                self.generate_and_send(&text, output)?;
                self.text_buffer.clear();
            }
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        if !self.text_buffer.is_empty() {
            plugin_warn!(self.logger, "Text buffer not empty at cleanup");
        }
    }
}

impl PocketTtsNode {
    fn handle_text(&mut self, packet: &Packet, output: &OutputSender) -> Result<(), String> {
        let text: Cow<'_, str> = match packet {
            Packet::Text(text) => Cow::Borrowed(text.as_ref()),
            Packet::Binary { data, .. } => Cow::Owned(
                String::from_utf8(data.to_vec())
                    .map_err(|e| format!("Failed to decode binary text as UTF-8: {e}"))?,
            ),
            _ => return Err("Only accepts Text or Binary packets on text input".to_string()),
        };

        let mut sanitized = Self::sanitize_text(text.as_ref());
        if sanitized.is_empty() {
            return Ok(());
        }

        if !sanitized.ends_with('.') && !sanitized.ends_with('!') && !sanitized.ends_with('?') {
            sanitized.push('.');
        }

        self.text_buffer.push_str(&sanitized);

        if self.voice_input_seen && !self.voice_ready {
            plugin_debug!(
                self.logger,
                "Voice prompt pending; buffering text (buffer_len={})",
                self.text_buffer.len()
            );
            return Ok(());
        }

        self.flush_text_buffer(output)
    }

    fn handle_voice(&mut self, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Binary { data, .. } => {
                if data.is_empty() {
                    return Ok(());
                }

                self.voice_input_seen = true;
                if self.voice_ready {
                    self.voice_buffer.clear();
                    self.voice_expected_len = None;
                    self.voice_ready = false;
                }

                self.voice_buffer.extend_from_slice(&data);
                self.update_voice_expected_len();

                if let Some(expected) = self.voice_expected_len {
                    if self.voice_buffer.len() >= expected {
                        self.load_voice_from_buffer()?;
                        if !self.text_buffer.is_empty() {
                            self.flush_text_buffer(output)?;
                        }
                    }
                }
                Ok(())
            },
            Packet::Text(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Ok(());
                }

                let voice_state = voice_state_from_base64(&self.model, trimmed).map_err(|e| {
                    plugin_error!(self.logger, "Voice base64 decode failed: {e}");
                    e
                })?;

                self.voice_input_seen = true;
                self.voice_state = Arc::new(voice_state);
                self.voice_ready = true;
                self.voice_buffer.clear();
                self.voice_expected_len = None;

                plugin_info!(self.logger, "Loaded voice prompt from text input");

                if !self.text_buffer.is_empty() {
                    self.flush_text_buffer(output)?;
                }
                Ok(())
            },
            _ => Err("Voice input must be Binary (wav) or Text (base64)".to_string()),
        }
    }

    fn update_voice_expected_len(&mut self) {
        if self.voice_expected_len.is_some() {
            return;
        }
        self.voice_expected_len = Self::read_wav_expected_len(&self.voice_buffer);
    }

    fn read_wav_expected_len(bytes: &[u8]) -> Option<usize> {
        if bytes.len() < 12 {
            return None;
        }

        if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return None;
        }

        let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        if riff_size == 0 {
            return None;
        }

        let expected = riff_size.saturating_add(8);
        if expected < 12 {
            return None;
        }

        Some(expected)
    }

    fn load_voice_from_buffer(&mut self) -> Result<(), String> {
        if self.voice_buffer.is_empty() {
            return Err("Voice prompt buffer is empty".to_string());
        }

        let bytes = if let Some(expected) = self.voice_expected_len {
            if self.voice_buffer.len() < expected {
                return Err(format!(
                    "Voice prompt incomplete: {}/{} bytes",
                    self.voice_buffer.len(),
                    expected
                ));
            }
            if self.voice_buffer.len() > expected {
                plugin_warn!(
                    self.logger,
                    "Voice prompt larger than expected ({} > {} bytes)",
                    self.voice_buffer.len(),
                    expected
                );
            }
            &self.voice_buffer[..expected]
        } else {
            &self.voice_buffer[..]
        };

        let voice_state = voice_state_from_wav_bytes(&self.model, bytes).map_err(|e| {
            plugin_error!(self.logger, "Voice prompt decode failed: {e}");
            e
        })?;

        self.voice_state = Arc::new(voice_state);
        self.voice_ready = true;
        self.voice_buffer.clear();
        self.voice_expected_len = None;

        plugin_info!(self.logger, "Loaded voice prompt from multipart input");
        Ok(())
    }

    fn flush_text_buffer(&mut self, output: &OutputSender) -> Result<(), String> {
        while let Some(sentence) = self.sentence_splitter.extract_sentence(&mut self.text_buffer) {
            self.generate_and_send(&sentence, output)?;
        }
        Ok(())
    }

    fn generate_and_send(&self, text: &str, output: &OutputSender) -> Result<(), String> {
        let voice_state = self.voice_state.as_ref();
        for chunk in self.model.generate_stream_long(text, voice_state) {
            let chunk = chunk.map_err(|e| format!("TTS generation failed: {e}"))?;
            let (samples, channels) = Self::tensor_to_interleaved_samples(&chunk)?;
            if samples.is_empty() {
                continue;
            }

            let sample_rate = u32::try_from(self.model.sample_rate).map_err(|_| {
                format!("Model sample rate {} does not fit in u32", self.model.sample_rate)
            })?;
            let frame = AudioFrame::new(sample_rate, channels, samples);
            output
                .send("out", &Packet::Audio(frame))
                .map_err(|e| format!("Failed to send audio: {e}"))?;
        }
        Ok(())
    }

    fn tensor_to_interleaved_samples(
        chunk: &candle_core::Tensor,
    ) -> Result<(Vec<f32>, u16), String> {
        let chunk = chunk.squeeze(0).map_err(|e| format!("Failed to squeeze audio tensor: {e}"))?;
        let data =
            chunk.to_vec2::<f32>().map_err(|e| format!("Failed to read audio tensor data: {e}"))?;

        if data.is_empty() {
            return Ok((Vec::new(), 1));
        }

        let channels = data.len();
        let samples_per_channel = data[0].len();
        for channel in &data {
            if channel.len() != samples_per_channel {
                return Err("Inconsistent channel lengths in audio tensor".to_string());
            }
        }

        let mut interleaved = Vec::with_capacity(channels * samples_per_channel);
        for i in 0..samples_per_channel {
            for channel in &data {
                interleaved.push(channel[i].clamp(-1.0, 1.0));
            }
        }

        let channels_u16 = u16::try_from(channels)
            .map_err(|_| format!("Channel count {channels} does not fit in u16"))?;

        Ok((interleaved, channels_u16))
    }

    fn sanitize_text(text: &str) -> String {
        text.replace(['\n', '\r', '\t'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
