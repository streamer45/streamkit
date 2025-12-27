// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SplitMode {
    /// Split on sentence boundaries (. ! ? etc.)
    Sentences,
    /// Split on sentences AND pauses (commas, dashes, semicolons) for natural streaming
    #[default]
    Clauses,
    /// Split after N words for lower latency (not recommended for TTS)
    Words,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TextChunkerConfig {
    /// Splitting mode: "sentences" or "words"
    pub split_mode: SplitMode,
    /// Minimum chunk length before emitting (used in sentence mode)
    pub min_length: usize,
    /// Number of words per chunk (used in word mode)
    pub chunk_words: usize,
}

impl Default for TextChunkerConfig {
    fn default() -> Self {
        Self { split_mode: SplitMode::Sentences, min_length: 10, chunk_words: 5 }
    }
}

/// Splits incoming text into chunks (sentences or word groups) for streaming TTS generation
pub struct TextChunkerNode {
    config: TextChunkerConfig,
    buffer: String,
}

impl TextChunkerNode {
    /// Creates a new text chunking node from configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration parameters cannot be parsed.
    pub fn new(params: Option<&serde_json::Value>) -> Result<Self, StreamKitError> {
        let config: TextChunkerConfig = config_helpers::parse_config_optional(params)?;
        Ok(Self { config, buffer: String::new() })
    }

    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| Ok(Box::new(Self::new(params)?)))
    }

    fn extract_sentence(&mut self) -> Option<String> {
        if self.buffer.len() < self.config.min_length {
            return None;
        }

        let boundaries = [". ", ".\n", "! ", "!\n", "? ", "?\n", "。", "！", "？"];

        for boundary in &boundaries {
            if let Some(pos) = self.buffer.find(boundary) {
                let end_pos = pos + boundary.len();
                let sentence: String = self.buffer.drain(..end_pos).collect();
                return Some(sentence.trim().to_string());
            }
        }

        if self.buffer.ends_with('.')
            || self.buffer.ends_with('!')
            || self.buffer.ends_with('?')
            || self.buffer.ends_with('。')
            || self.buffer.ends_with('！')
            || self.buffer.ends_with('？')
        {
            let sentence = self.buffer.drain(..).collect();
            return Some(sentence);
        }

        None
    }

    fn extract_word_chunk(&mut self) -> Option<String> {
        // Count words in buffer
        if self.buffer.split_whitespace().count() < self.config.chunk_words {
            return None;
        }

        // Find position after N words
        let mut word_count = 0;
        let mut last_word_end = 0;

        for (idx, ch) in self.buffer.char_indices() {
            if ch.is_whitespace() && idx > last_word_end {
                word_count += 1;
                if word_count >= self.config.chunk_words {
                    // Extract up to this point
                    let chunk: String = self.buffer.drain(..=idx).collect();
                    self.buffer = self.buffer.trim_start().to_string();
                    return Some(chunk.trim().to_string());
                }
                last_word_end = idx;
            }
        }

        // If we have exactly chunk_words and no trailing whitespace
        if word_count == self.config.chunk_words - 1 && !self.buffer.is_empty() {
            let chunk = self.buffer.drain(..).collect();
            return Some(chunk);
        }

        None
    }

    fn extract_clause(&mut self) -> Option<String> {
        if self.buffer.len() < self.config.min_length {
            return None;
        }

        // Split on natural pauses: sentence endings, commas, semicolons, dashes, colons
        // This creates natural chunks for TTS while maintaining intonation
        let boundaries = [
            ". ", ".\n", "! ", "!\n", "? ", "?\n", // Sentence endings (English)
            "。", "！", "？", // Sentence endings (Chinese)
            ", ", ",\n", // Commas (natural pauses)
            "; ", ";\n", // Semicolons
            " - ", " – ", " — ", // Dashes (with spaces)
            ": ", ":\n", // Colons (list introductions)
        ];

        for boundary in &boundaries {
            if let Some(pos) = self.buffer.find(boundary) {
                let end_pos = pos + boundary.len();
                let clause: String = self.buffer.drain(..end_pos).collect();
                return Some(clause.trim().to_string());
            }
        }

        // Check for final punctuation at end (no trailing space/newline)
        if self.buffer.ends_with('.')
            || self.buffer.ends_with('!')
            || self.buffer.ends_with('?')
            || self.buffer.ends_with('。')
            || self.buffer.ends_with('！')
            || self.buffer.ends_with('？')
            || self.buffer.ends_with(',')
            || self.buffer.ends_with(';')
            || self.buffer.ends_with(':')
        {
            let clause = self.buffer.drain(..).collect();
            return Some(clause);
        }

        None
    }

    fn extract_chunk(&mut self) -> Option<String> {
        match self.config.split_mode {
            SplitMode::Sentences => self.extract_sentence(),
            SplitMode::Clauses => self.extract_clause(),
            SplitMode::Words => self.extract_word_chunk(),
        }
    }
}

#[async_trait]
impl ProcessorNode for TextChunkerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Text, PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Text,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("TextChunkerNode starting (mode: {:?})", self.config.split_mode);
        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;
        let mut chunk_count = 0;

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            let text: std::borrow::Cow<'_, str> = match &packet {
                Packet::Text(t) => std::borrow::Cow::Borrowed(t.as_ref()),
                Packet::Binary { data, .. } => std::borrow::Cow::Owned(
                    String::from_utf8(data.to_vec())
                        .map_err(|e| StreamKitError::Runtime(format!("Invalid UTF-8: {e}")))?,
                ),
                _ => continue,
            };

            if text.is_empty() {
                continue;
            }

            self.buffer.push_str(text.as_ref());
            tracing::debug!(
                buffer_size = self.buffer.len(),
                buffer_preview = %self.buffer.chars().take(100).collect::<String>(),
                "Buffer after adding text"
            );

            while let Some(chunk) = self.extract_chunk() {
                chunk_count += 1;
                tracing::debug!(
                    chunk_count,
                    chunk_len = chunk.len(),
                    chunk_text = %chunk,
                    remaining_buffer_size = self.buffer.len(),
                    "Emitting chunk"
                );

                if context.output_sender.send("out", Packet::Text(chunk.into())).await.is_err() {
                    tracing::debug!("Output closed");
                    break;
                }
            }

            if !self.buffer.is_empty() {
                tracing::debug!(
                    remaining_buffer_size = self.buffer.len(),
                    remaining_preview = %self.buffer.chars().take(100).collect::<String>(),
                    "Text remains in buffer after extraction"
                );
            }
        }

        if !self.buffer.is_empty() {
            let remaining: String = self.buffer.drain(..).collect();
            tracing::info!(
                remaining_len = remaining.len(),
                remaining_text = %remaining,
                "Flushing remaining buffer"
            );
            let _ = context.output_sender.send("out", Packet::Text(remaining.into())).await;
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed".to_string());
        tracing::info!("TextChunkerNode finished, emitted {} chunks", chunk_count);
        Ok(())
    }
}
