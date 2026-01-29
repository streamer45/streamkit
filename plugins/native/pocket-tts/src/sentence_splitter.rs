// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

pub struct SentenceSplitter {
    min_length: usize,
}

impl SentenceSplitter {
    pub const fn new(min_length: usize) -> Self {
        Self { min_length }
    }

    /// Extract complete sentence from buffer if available.
    pub fn extract_sentence(&self, buffer: &mut String) -> Option<String> {
        if buffer.len() < self.min_length {
            return None;
        }

        let boundaries = [
            ". ", ".\n", "! ", "!\n", "? ", "?\n", // English
        ];

        for boundary in &boundaries {
            if let Some(pos) = buffer.find(boundary) {
                let end_pos = pos + boundary.len();
                let sentence: String = buffer.drain(..end_pos).collect();
                return Some(sentence.trim().to_string());
            }
        }

        if buffer.ends_with('.') || buffer.ends_with('!') || buffer.ends_with('?') {
            let sentence = std::mem::take(buffer);
            return Some(sentence);
        }

        None
    }

    #[allow(dead_code)]
    pub fn flush(buffer: &mut String) -> Option<String> {
        if buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(buffer))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sentence_extraction() {
        let splitter = SentenceSplitter::new(5);
        let mut buffer = "Hello world. How are you?".to_string();

        assert_eq!(splitter.extract_sentence(&mut buffer), Some("Hello world.".to_string()));
        assert_eq!(buffer, "How are you?");

        assert_eq!(splitter.extract_sentence(&mut buffer), Some("How are you?".to_string()));
        assert_eq!(buffer, "");
    }

    #[test]
    fn test_min_length() {
        let splitter = SentenceSplitter::new(20);
        let mut buffer = "Hi.".to_string();

        assert_eq!(splitter.extract_sentence(&mut buffer), None);
        assert_eq!(buffer, "Hi.");
    }
}
