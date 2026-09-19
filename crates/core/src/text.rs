// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Incremental text segmentation shared by the text-chunker node and the
//! TTS plugins that buffer streamed text into whole sentences.

/// Boundaries ending a complete sentence: English punctuation followed by
/// whitespace, or CJK punctuation (which needs no trailing whitespace).
pub const SENTENCE_BOUNDARIES: &[&str] = &[". ", ".\n", "! ", "!\n", "? ", "?\n", "。", "！", "？"];

/// Punctuation ending a sentence even without trailing whitespace.
pub const SENTENCE_TRAILING: &[char] = &['.', '!', '?', '。', '！', '？'];

/// Drain the first complete chunk from the front of `buffer`.
///
/// `boundaries` are tried in table order (not buffer position); on the first
/// match the buffer up to and including the boundary is drained, trimmed, and
/// returned. With no boundary match, a `trailing` punctuation char at the end
/// of `buffer` completes the chunk. `None` is returned while `buffer` is
/// shorter than `min_length` or contains no completed chunk.
pub fn extract_chunk(
    buffer: &mut String,
    min_length: usize,
    boundaries: &[&str],
    trailing: &[char],
) -> Option<String> {
    if buffer.len() < min_length {
        return None;
    }

    for boundary in boundaries {
        if let Some(pos) = buffer.find(boundary) {
            let end_pos = pos + boundary.len();
            let chunk: String = buffer.drain(..end_pos).collect();
            return Some(chunk.trim().to_string());
        }
    }

    if trailing.iter().any(|&p| buffer.ends_with(p)) {
        return Some(std::mem::take(buffer));
    }

    None
}

/// Splits a growing text buffer into complete sentences on English or CJK
/// punctuation boundaries.
pub struct SentenceSplitter {
    min_length: usize,
}

impl SentenceSplitter {
    pub const fn new(min_length: usize) -> Self {
        Self { min_length }
    }

    /// Drain and return the first complete sentence in `buffer`, if it is at
    /// least `min_length` long and reaches a sentence boundary.
    pub fn extract_sentence(&self, buffer: &mut String) -> Option<String> {
        extract_chunk(buffer, self.min_length, SENTENCE_BOUNDARIES, SENTENCE_TRAILING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sentence_english_boundaries() {
        let splitter = SentenceSplitter::new(5);
        let mut buffer = "Hello world. How are you?".to_string();

        assert_eq!(splitter.extract_sentence(&mut buffer), Some("Hello world.".to_string()));
        assert_eq!(buffer, "How are you?");

        assert_eq!(splitter.extract_sentence(&mut buffer), Some("How are you?".to_string()));
        assert!(buffer.is_empty());
    }

    #[test]
    fn extract_sentence_cjk_boundaries() {
        let splitter = SentenceSplitter::new(1);
        let mut buffer = "こんにちは。元気ですか？".to_string();

        assert_eq!(splitter.extract_sentence(&mut buffer), Some("こんにちは。".to_string()));
        assert_eq!(splitter.extract_sentence(&mut buffer), Some("元気ですか？".to_string()));
        assert!(buffer.is_empty());
    }

    #[test]
    fn extract_sentence_respects_min_length() {
        let splitter = SentenceSplitter::new(20);
        let mut buffer = "Hi.".to_string();

        assert_eq!(splitter.extract_sentence(&mut buffer), None);
        assert_eq!(buffer, "Hi.");
    }

    #[test]
    fn extract_chunk_custom_boundaries() {
        let mut buffer = "first clause, rest".to_string();

        assert_eq!(
            extract_chunk(&mut buffer, 1, &[", "], &[',']),
            Some("first clause,".to_string())
        );
        assert_eq!(buffer, "rest");
    }
}
