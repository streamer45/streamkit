// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{AudioFormat, SampleFormat};

use crate::config::SupertonicConfig;
use crate::model::{self, TtsModelWrapper};
use crate::sentence_splitter::SentenceSplitter;
use crate::voice::{self, StyleWrapper};

pub struct SupertonicNode {
    tts_model: Arc<TtsModelWrapper>,
    voice_style: Arc<StyleWrapper>,
    config: SupertonicConfig,
    model_dir: String,
    sample_rate: i32,
    text_buffer: String,
    sentence_splitter: SentenceSplitter,
    logger: Logger,
}

// SAFETY: Thread-safety is ensured through Arc and Mutex on shared resources.
unsafe impl Send for SupertonicNode {}
unsafe impl Sync for SupertonicNode {}

impl NativeProcessorNode for SupertonicNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("supertonic")
            .description(
                "Multilingual text-to-speech using the Supertonic TTS engine. \
                 Supports 5 languages (en, ko, es, pt, fr) with 10 voice styles. \
                 66M parameters, up to 167x faster than real-time.",
            )
            .input("in", &[PacketType::Text])
            .output(
                "out",
                PacketType::RawAudio(AudioFormat {
                    sample_rate: 22050,
                    channels: 1,
                    sample_format: SampleFormat::F32,
                }),
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "model_dir": {
                        "type": "string",
                        "description": "Path to Supertonic ONNX model directory",
                        "default": "./models/supertonic-v2-onnx"
                    },
                    "lang": {
                        "type": "string",
                        "description": "Language code",
                        "default": "en",
                        "enum": ["en", "ko", "es", "pt", "fr"]
                    },
                    "voice_style": {
                        "type": "string",
                        "description": "Voice style name (M1-M5, F1-F5) or path to .json file",
                        "default": "M1"
                    },
                    "voice_styles_dir": {
                        "type": "string",
                        "description": "Directory containing named voice style .json files"
                    },
                    "total_step": {
                        "type": "integer",
                        "description": "Denoising steps (higher = better quality, slower)",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 20
                    },
                    "speed": {
                        "type": "number",
                        "description": "Speech speed multiplier",
                        "default": 1.05,
                        "minimum": 0.5,
                        "maximum": 2.0
                    },
                    "silence_duration": {
                        "type": "number",
                        "description": "Silence between chunks in seconds",
                        "default": 0.3,
                        "minimum": 0.0,
                        "maximum": 2.0
                    },
                    "min_sentence_length": {
                        "type": "integer",
                        "description": "Minimum chars before TTS generation",
                        "default": 10,
                        "minimum": 1
                    },
                    "emit_telemetry": {
                        "type": "boolean",
                        "description": "Emit out-of-band telemetry events (tts.start/tts.done)",
                        "default": false
                    },
                    "telemetry_preview_chars": {
                        "type": "integer",
                        "description": "Maximum characters of text preview in telemetry (0 = omit)",
                        "default": 80,
                        "minimum": 0,
                        "maximum": 1000
                    }
                },
                "required": ["model_dir"]
            }))
            .category("audio")
            .category("tts")
            .category("ml")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        plugin_info!(logger, "SupertonicNode::new() called with params: {:?}", params);

        let config: SupertonicConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?
        } else {
            SupertonicConfig::default()
        };

        plugin_info!(
            logger,
            "Config: model_dir={}, lang={}, voice_style={}, total_step={}, speed={}",
            config.model_dir,
            config.lang,
            config.voice_style,
            config.total_step,
            config.speed
        );

        // Canonicalize model path
        let model_dir = PathBuf::from(&config.model_dir);
        let model_dir = if model_dir.is_absolute() {
            model_dir
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Failed to get current dir: {e}"))?
                .join(model_dir)
        };
        let model_dir = model_dir.canonicalize().map_err(|e| {
            format!("Failed to canonicalize model dir '{}': {e}", model_dir.display())
        })?;
        let model_dir_str = model_dir.to_string_lossy().to_string();

        plugin_info!(logger, "Canonicalized model_dir: {}", model_dir_str);

        // Load/cache model
        let (tts_model, sample_rate) = model::get_or_load_model(&model_dir_str, &logger)?;

        plugin_info!(logger, "Model loaded, sample_rate={}", sample_rate);

        // Load/cache voice style
        let voice_style = voice::resolve_voice_style(
            &config.voice_style,
            config.voice_styles_dir.as_deref(),
            &model_dir_str,
            &logger,
        )?;

        let min_sentence_length = config.min_sentence_length;

        Ok(Self {
            tts_model,
            voice_style,
            config,
            model_dir: model_dir_str,
            sample_rate,
            text_buffer: String::new(),
            sentence_splitter: SentenceSplitter::new(min_sentence_length),
            logger,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        let text: std::borrow::Cow<'_, str> = match &packet {
            Packet::Text(text) => std::borrow::Cow::Borrowed(text.as_ref()),
            Packet::Binary { data, .. } => std::borrow::Cow::Owned(
                String::from_utf8(data.to_vec())
                    .map_err(|e| format!("Failed to decode binary data as UTF-8: {e}"))?,
            ),
            _ => return Err("Only accepts Text or Binary packets".to_string()),
        };

        plugin_debug!(self.logger, text = %text, "Received text input");

        let mut sanitized = Self::sanitize_text(text.as_ref());

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
        }

        self.text_buffer.push_str(&sanitized);

        while let Some(sentence) = self.sentence_splitter.extract_sentence(&mut self.text_buffer) {
            plugin_info!(self.logger, sentence_len = sentence.len(), "Generating TTS for sentence");
            self.generate_and_send(&sentence, output)?;
        }

        Ok(())
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        if let Some(p) = params {
            let new_config: SupertonicConfig =
                serde_json::from_value(p).map_err(|e| format!("Config parse error: {e}"))?;

            // Hot-update runtime parameters
            self.config.lang = new_config.lang;
            self.config.total_step = new_config.total_step;
            self.config.speed = new_config.speed;
            self.config.silence_duration = new_config.silence_duration;
            self.config.min_sentence_length = new_config.min_sentence_length;
            self.config.emit_telemetry = new_config.emit_telemetry;
            self.config.telemetry_preview_chars = new_config.telemetry_preview_chars;

            // Reload voice style if changed
            if new_config.voice_style != self.config.voice_style
                || new_config.voice_styles_dir != self.config.voice_styles_dir
            {
                plugin_info!(
                    self.logger,
                    old = %self.config.voice_style,
                    new = %new_config.voice_style,
                    "Voice style changed, reloading"
                );
                self.voice_style = voice::resolve_voice_style(
                    &new_config.voice_style,
                    new_config.voice_styles_dir.as_deref(),
                    &self.model_dir,
                    &self.logger,
                )?;
                self.config.voice_style = new_config.voice_style;
                self.config.voice_styles_dir = new_config.voice_styles_dir;
            }

            // Warn if model_dir changed (requires node recreation)
            if new_config.model_dir != self.config.model_dir {
                plugin_warn!(
                    self.logger,
                    "model_dir changed but requires node recreation to take effect"
                );
            }

            self.sentence_splitter = SentenceSplitter::new(self.config.min_sentence_length);
        }

        Ok(())
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_info!(
            self.logger,
            buffer_len = self.text_buffer.len(),
            "Flush called on Supertonic TTS"
        );

        if self.text_buffer.is_empty() {
            plugin_info!(self.logger, "Text buffer was empty during flush");
        } else {
            let text = self.text_buffer.clone();
            plugin_info!(self.logger, len = text.len(), "Flushing remaining text buffer");
            self.generate_and_send(&text, output)?;
            self.text_buffer.clear();
        }

        Ok(())
    }

    fn cleanup(&mut self) {
        if !self.text_buffer.is_empty() {
            plugin_warn!(
                self.logger,
                len = self.text_buffer.len(),
                "Text buffer not empty at cleanup"
            );
        }
    }
}

impl SupertonicNode {
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

    fn generate_and_send(&self, text: &str, output: &OutputSender) -> Result<(), String> {
        plugin_debug!(self.logger, text_len = text.len(), "Starting TTS generation");

        let start = Instant::now();
        if self.config.emit_telemetry {
            let _ = output.emit_telemetry(
                "tts.start",
                &serde_json::json!({
                    "text_length": text.len(),
                    "text_preview": self.text_preview(text),
                    "lang": self.config.lang,
                    "voice_style": self.config.voice_style,
                    "speed": self.config.speed,
                    "total_step": self.config.total_step,
                }),
                None,
            );
        }

        // Lock the model for inference (TextToSpeech::call takes &mut self)
        let (wav, _duration) = {
            let mut tts = self.tts_model.lock()?;
            tts.call(
                text,
                &self.config.lang,
                &self.voice_style.0,
                self.config.total_step,
                self.config.speed,
                self.config.silence_duration,
            )
            .map_err(|e| format!("TTS generation failed: {e}"))?
        };

        if wav.is_empty() {
            plugin_warn!(self.logger, "TTS generated empty audio");
            return Err("TTS generated empty audio".to_string());
        }

        let sample_count = wav.len();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frame = AudioFrame::new(self.sample_rate as u32, 1, wav);

        output.send("out", &Packet::Audio(frame)).map_err(|e| {
            plugin_error!(self.logger, error = %e, "Failed to send audio frame");
            format!("Failed to send audio: {e}")
        })?;

        plugin_debug!(self.logger, sample_count = sample_count, "Sent audio frame");

        if self.config.emit_telemetry {
            let latency_ms = start.elapsed().as_millis();
            #[allow(clippy::cast_sign_loss)]
            let sr = self.sample_rate as u64;
            let duration_ms = if sr > 0 { (sample_count as u64 * 1000 + sr / 2) / sr } else { 0 };
            let _ = output.emit_telemetry(
                "tts.done",
                &serde_json::json!({
                    "text_length": text.len(),
                    "text_preview": self.text_preview(text),
                    "lang": self.config.lang,
                    "voice_style": self.config.voice_style,
                    "speed": self.config.speed,
                    "total_step": self.config.total_step,
                    "audio_samples": sample_count,
                    "audio_duration_ms": duration_ms,
                    "latency_ms": latency_ms,
                }),
                None,
            );
        }

        Ok(())
    }

    /// Sanitize text input: keep alphanumeric, punctuation, Korean/CJK, accented Latin, whitespace
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
                // CJK unified ideographs
                | '\u{4E00}'..='\u{9FFF}'
                // Korean Hangul syllables
                | '\u{AC00}'..='\u{D7AF}'
                // Korean Hangul Jamo
                | '\u{1100}'..='\u{11FF}'
                // Korean Hangul Compatibility Jamo
                | '\u{3130}'..='\u{318F}'
                // CJK punctuation
                | '。' | '，' | '！' | '？' | '、' | '；' | '：' | '（' | '）' => Some(c),
                c if c.is_whitespace() => Some(' '),
                _ => None,
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Drop for SupertonicNode {
    fn drop(&mut self) {
        // Arc references will be dropped automatically
    }
}
