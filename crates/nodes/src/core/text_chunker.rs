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
#[serde(default, deny_unknown_fields)]
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

pub struct TextChunkerNode {
    config: TextChunkerConfig,
    buffer: String,
}

impl TextChunkerNode {
    /// # Errors
    /// Returns `Err` if config parsing fails.
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
        if self.buffer.split_whitespace().count() < self.config.chunk_words {
            return None;
        }

        let mut word_count = 0;
        let mut last_word_end = 0;

        for (idx, ch) in self.buffer.char_indices() {
            if ch.is_whitespace() && idx > last_word_end {
                word_count += 1;
                if word_count >= self.config.chunk_words {
                    let chunk: String = self.buffer.drain(..=idx).collect();
                    self.buffer = self.buffer.trim_start().to_string();
                    return Some(chunk.trim().to_string());
                }
                last_word_end = idx;
            }
        }

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Tests use unwrap/expect for concise assertions.
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    fn make_node(split_mode: SplitMode, min_length: usize, chunk_words: usize) -> TextChunkerNode {
        TextChunkerNode {
            config: TextChunkerConfig { split_mode, min_length, chunk_words },
            buffer: String::new(),
        }
    }

    #[test]
    fn new_default_config() {
        let node = TextChunkerNode::new(None).unwrap();
        assert!(matches!(node.config.split_mode, SplitMode::Sentences));
        assert_eq!(node.config.min_length, 10);
        assert_eq!(node.config.chunk_words, 5);
    }

    #[test]
    fn new_explicit_config() {
        let params = serde_json::json!({
            "split_mode": "words",
            "min_length": 20,
            "chunk_words": 3
        });
        let node = TextChunkerNode::new(Some(&params)).unwrap();
        assert!(matches!(node.config.split_mode, SplitMode::Words));
        assert_eq!(node.config.min_length, 20);
        assert_eq!(node.config.chunk_words, 3);
    }

    #[test]
    fn factory_returns_node() {
        let factory = TextChunkerNode::factory();
        assert!(factory(None).is_ok());
    }

    #[test]
    fn sentence_below_min_length_returns_none() {
        let mut node = make_node(SplitMode::Sentences, 20, 5);
        node.buffer = "Hi. ".to_string();
        assert!(node.extract_sentence().is_none());
    }

    #[test]
    fn sentence_period_space_boundary() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "Hello world. More text".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "Hello world.");
        assert_eq!(node.buffer, "More text");
    }

    #[test]
    fn sentence_exclamation_boundary() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "Wow! Next".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "Wow!");
    }

    #[test]
    fn sentence_question_newline_boundary() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "Really?\nYes".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "Really?");
    }

    #[test]
    fn sentence_trailing_period() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "End of text.".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "End of text.");
        assert!(node.buffer.is_empty());
    }

    #[test]
    fn sentence_trailing_cjk_punctuation() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "日本語のテスト。".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "日本語のテスト。");
    }

    #[test]
    fn sentence_no_boundary_returns_none() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "no boundary here".to_string();
        assert!(node.extract_sentence().is_none());
    }

    #[test]
    fn sentence_multi_sentence_buffer() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "First. Second. Third".to_string();
        assert_eq!(node.extract_sentence().unwrap(), "First.");
        assert_eq!(node.extract_sentence().unwrap(), "Second.");
        assert!(node.extract_sentence().is_none());
        assert_eq!(node.buffer, "Third");
    }

    #[test]
    fn clause_below_min_length_returns_none() {
        let mut node = make_node(SplitMode::Clauses, 50, 5);
        node.buffer = "Short, text".to_string();
        assert!(node.extract_clause().is_none());
    }

    #[test]
    fn clause_comma_boundary() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "first clause, second clause".to_string();
        assert_eq!(node.extract_clause().unwrap(), "first clause,");
    }

    #[test]
    fn clause_semicolon_boundary() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "before; after".to_string();
        assert_eq!(node.extract_clause().unwrap(), "before;");
    }

    #[test]
    fn clause_dash_boundary() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "one thing — another thing".to_string();
        assert_eq!(node.extract_clause().unwrap(), "one thing —");
    }

    #[test]
    fn clause_colon_boundary() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "items: apples".to_string();
        assert_eq!(node.extract_clause().unwrap(), "items:");
    }

    #[test]
    fn clause_trailing_comma() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "trailing comma,".to_string();
        assert_eq!(node.extract_clause().unwrap(), "trailing comma,");
        assert!(node.buffer.is_empty());
    }

    #[test]
    fn clause_trailing_semicolon() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "trailing semicolon;".to_string();
        assert_eq!(node.extract_clause().unwrap(), "trailing semicolon;");
    }

    #[test]
    fn clause_trailing_colon() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "trailing colon:".to_string();
        assert_eq!(node.extract_clause().unwrap(), "trailing colon:");
    }

    #[test]
    fn clause_also_splits_on_sentence_boundaries() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "Hello world. More".to_string();
        assert_eq!(node.extract_clause().unwrap(), "Hello world.");
    }

    #[test]
    fn word_chunk_below_threshold_returns_none() {
        let mut node = make_node(SplitMode::Words, 1, 5);
        node.buffer = "one two three".to_string();
        assert!(node.extract_word_chunk().is_none());
    }

    #[test]
    fn word_chunk_exact_threshold() {
        let mut node = make_node(SplitMode::Words, 1, 3);
        node.buffer = "one two three".to_string();
        let chunk = node.extract_word_chunk().unwrap();
        assert_eq!(chunk, "one two three");
    }

    #[test]
    fn word_chunk_above_threshold() {
        let mut node = make_node(SplitMode::Words, 1, 3);
        node.buffer = "one two three four five".to_string();
        let chunk = node.extract_word_chunk().unwrap();
        assert_eq!(chunk, "one two three");
        assert_eq!(node.buffer, "four five");
    }

    #[test]
    fn word_chunk_multiple_extractions() {
        let mut node = make_node(SplitMode::Words, 1, 2);
        node.buffer = "a b c d e".to_string();
        assert_eq!(node.extract_word_chunk().unwrap(), "a b");
        assert_eq!(node.extract_word_chunk().unwrap(), "c d");
        assert!(node.extract_word_chunk().is_none());
        assert_eq!(node.buffer, "e");
    }

    #[test]
    fn extract_chunk_dispatches_to_sentence_mode() {
        let mut node = make_node(SplitMode::Sentences, 1, 5);
        node.buffer = "Hello world. More".to_string();
        assert_eq!(node.extract_chunk().unwrap(), "Hello world.");
    }

    #[test]
    fn extract_chunk_dispatches_to_clause_mode() {
        let mut node = make_node(SplitMode::Clauses, 1, 5);
        node.buffer = "first clause, rest".to_string();
        assert_eq!(node.extract_chunk().unwrap(), "first clause,");
    }

    #[test]
    fn extract_chunk_dispatches_to_word_mode() {
        let mut node = make_node(SplitMode::Words, 1, 2);
        node.buffer = "alpha beta gamma".to_string();
        assert_eq!(node.extract_chunk().unwrap(), "alpha beta");
    }

    #[tokio::test]
    async fn run_chunks_text_and_flushes_remainder() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = TextChunkerNode::new(Some(&serde_json::json!({
            "split_mode": "sentences",
            "min_length": 1
        })))
        .unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("First sentence. Second".into())).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(packets.len(), 1);
        match &packets[0] {
            Packet::Text(t) => assert_eq!(t.as_ref(), "First sentence."),
            _ => panic!("Expected text packet"),
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let remaining = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(remaining.len(), 1);
        match &remaining[0] {
            Packet::Text(t) => assert_eq!(t.as_ref(), "Second"),
            _ => panic!("Expected text packet"),
        }
    }

    #[tokio::test]
    async fn run_handles_binary_input_as_utf8() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = TextChunkerNode::new(Some(&serde_json::json!({
            "split_mode": "sentences",
            "min_length": 1
        })))
        .unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let binary_text = Packet::Binary {
            data: bytes::Bytes::from("Done."),
            content_type: None,
            metadata: None,
        };
        input_tx.send(binary_text).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(packets.len(), 1);
        match &packets[0] {
            Packet::Text(t) => assert_eq!(t.as_ref(), "Done."),
            _ => panic!("Expected text packet"),
        }
    }

    #[tokio::test]
    async fn run_skips_empty_text() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = TextChunkerNode::new(Some(&serde_json::json!({
            "split_mode": "sentences",
            "min_length": 1
        })))
        .unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("".into())).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        assert!(packets.is_empty());
    }
}
