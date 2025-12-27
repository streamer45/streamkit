// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Silero VAD v6 wrapper for voice activity detection
//!
//! This module provides a lightweight Rust wrapper around the Silero VAD v6 ONNX model
//! for detecting speech vs. silence in audio streams.

use ndarray::{Array1, Array2, Array3};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Value;

/// Silero VAD v6 for voice activity detection
///
/// Processes audio in 512-sample chunks (32ms @ 16kHz) and maintains RNN state
/// and context between frames for temporal continuity.
#[derive(Debug)]
pub struct SileroVAD {
    session: Session,
    sample_rate: u32,
    state: Array3<f32>, // RNN state [2, batch_size, 128] where batch_size=1
    context: Vec<f32>,  // Context samples from previous frame (64 samples for v6)
    threshold: f32,
}

impl SileroVAD {
    /// Create a new Silero VAD instance
    ///
    /// # Arguments
    /// * `model_path` - Path to the silero_vad.onnx model file
    /// * `sample_rate` - Audio sample rate (8000 or 16000)
    /// * `threshold` - Speech probability threshold (0.0-1.0, default 0.5)
    pub fn new(model_path: &str, sample_rate: u32, threshold: f32) -> Result<Self, String> {
        // Validate sample rate
        if sample_rate != 8000 && sample_rate != 16000 {
            return Err(format!("Silero VAD only supports 8kHz or 16kHz, got {sample_rate}Hz"));
        }

        // Load ONNX model
        let session = Session::builder()
            .map_err(|e| format!("Failed to create session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("Failed to set optimization level: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| format!("Failed to load VAD model from '{model_path}': {e}"))?;

        // Initialize RNN state [2, batch_size, 128] where batch_size=1
        let state = Array3::<f32>::zeros((2, 1, 128));

        // Initialize context buffer with 64 zeros (window_size / 8 = 512 / 8 = 64)
        let context = vec![0.0f32; 64];

        Ok(Self { session, sample_rate, state, context, threshold })
    }

    /// Process a 512-sample audio chunk and return speech probability
    ///
    /// Silero VAD v6 requires context from the previous frame for temporal continuity.
    /// The model expects [context_samples + window_samples] = [64 + 512] = 576 samples.
    ///
    /// # Arguments
    /// * `audio` - Audio samples (exactly 512 samples)
    ///
    /// # Returns
    /// Speech probability (0.0-1.0)
    pub fn process_chunk(&mut self, audio: &[f32]) -> Result<f32, String> {
        if audio.len() != 512 {
            return Err(format!("Silero VAD expects exactly 512 samples, got {}", audio.len()));
        }

        // Prepend context samples (64) to current audio (512) for effective window of 576
        let mut input_with_context = Vec::with_capacity(576);
        input_with_context.extend_from_slice(&self.context);
        input_with_context.extend_from_slice(audio);

        // Prepare input tensor with batch dimension: [batch_size, num_samples] = [1, 576]
        let audio_input = Array2::from_shape_vec((1, 576), input_with_context)
            .map_err(|e| format!("Failed to create audio input tensor: {e}"))?;

        // Sample rate as int64 scalar array
        let sr_input = Array1::from_vec(vec![i64::from(self.sample_rate)]);

        // Convert to ort::Value
        let input_value = Value::from_array(audio_input)
            .map_err(|e| format!("Failed to convert audio to Value: {e}"))?;

        let state_value = Value::from_array(self.state.clone())
            .map_err(|e| format!("Failed to convert state to Value: {e}"))?;

        let sr_value = Value::from_array(sr_input)
            .map_err(|e| format!("Failed to convert sample rate to Value: {e}"))?;

        // Run inference with inputs: input, state, sr
        let outputs = self
            .session
            .run(ort::inputs![input_value, state_value, sr_value])
            .map_err(|e| format!("VAD inference failed: {e}"))?;

        // Extract probability (first output)
        let prob_view = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract probability: {e}"))?;
        let probability = prob_view.1[0]; // Extract first element

        // Extract updated state (second output)
        let state_view = outputs[1]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract state: {e}"))?;
        let state_new = Array3::from_shape_vec((2, 1, 128), state_view.1.to_vec())
            .map_err(|e| format!("Failed to reshape state: {e}"))?;

        // Update state for next iteration
        self.state = state_new;

        // Update context: save last 64 samples of current audio for next frame
        self.context.copy_from_slice(&audio[audio.len() - 64..]);

        Ok(probability)
    }

    /// Check if audio chunk contains speech
    ///
    /// # Arguments
    /// * `audio` - Audio samples (exactly 512 samples)
    ///
    /// # Returns
    /// `true` if speech detected, `false` if silence
    #[allow(dead_code)]
    pub fn is_speech(&mut self, audio: &[f32]) -> Result<bool, String> {
        let probability = self.process_chunk(audio)?;
        Ok(probability >= self.threshold)
    }

    /// Reset VAD state (clears RNN state and context buffer)
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }

    /// Update speech threshold
    pub const fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.0, 1.0);
    }

    /// Get current threshold
    #[allow(dead_code)]
    pub const fn threshold(&self) -> f32 {
        self.threshold
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::expect_used)]
#[allow(clippy::disallowed_macros)]
#[allow(clippy::uninlined_format_args)]
#[allow(clippy::cast_precision_loss)]
#[allow(clippy::suboptimal_flops)]
#[allow(clippy::needless_range_loop)]
#[allow(clippy::cloned_instead_of_copied)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires model file"]
    fn test_vad_initialization() {
        let vad = SileroVAD::new("../../models/silero_vad.onnx", 16000, 0.5);
        assert!(vad.is_ok());
    }

    #[test]
    #[ignore = "Requires model file"]
    fn test_vad_silence_detection() {
        let mut vad = SileroVAD::new("../../models/silero_vad.onnx", 16000, 0.5).unwrap();

        // Test with silence (zeros)
        let silence = vec![0.0f32; 512];
        let probability = vad.process_chunk(&silence).unwrap();

        println!("Silence probability: {}", probability);
        // Silence should have low probability
        assert!(probability < 0.5);
    }

    #[test]
    #[ignore = "Requires model file"]
    fn test_vad_synthetic_speech() {
        let mut vad = SileroVAD::new("../../models/silero_vad.onnx", 16000, 0.5).unwrap();

        // Generate synthetic "speech-like" signal (sine wave with some variation)
        let mut audio = vec![0.0f32; 512];
        for i in 0..512 {
            let t = i as f32 / 16000.0;
            // Mix of frequencies to simulate speech formants
            audio[i] = 0.3 * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
                + 0.2 * (2.0 * std::f32::consts::PI * 800.0 * t).sin()
                + 0.1 * (2.0 * std::f32::consts::PI * 2000.0 * t).sin();
        }

        let probability = vad.process_chunk(&audio).unwrap();
        println!("Synthetic speech probability: {}", probability);
    }

    #[test]
    fn test_invalid_sample_rate() {
        let result = SileroVAD::new("../../models/silero_vad.onnx", 48000, 0.5);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only supports 8kHz or 16kHz"));
    }

    #[test]
    #[ignore = "Requires model file"]
    fn test_invalid_chunk_size() {
        let mut vad = SileroVAD::new("../../models/silero_vad.onnx", 16000, 0.5).unwrap();

        // Test with wrong size
        let audio = vec![0.0f32; 256];
        let result = vad.process_chunk(&audio);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expects exactly 512 samples"));
    }

    #[test]
    #[ignore = "Requires model file and audio file"]
    fn test_vad_with_real_audio() {
        // This test processes a real audio file through VAD
        // Usage: cargo test test_vad_with_real_audio -- --ignored --nocapture

        let mut vad = SileroVAD::new("../../models/silero_vad.onnx", 16000, 0.5).unwrap();

        // Try to load a test audio file
        let audio_path = std::env::var("TEST_AUDIO_PATH")
            .unwrap_or_else(|_| "../../samples/audio/test.raw".to_string());

        println!("Attempting to load audio from: {}", audio_path);

        match std::fs::read(&audio_path) {
            Ok(bytes) => {
                // Assume raw f32 samples
                let sample_count = bytes.len() / 4;
                let mut samples = vec![0.0f32; sample_count];

                for (i, chunk) in bytes.chunks_exact(4).enumerate() {
                    samples[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }

                println!(
                    "Loaded {} samples ({:.2}s of audio)",
                    sample_count,
                    sample_count as f32 / 16000.0
                );

                // Process in 512-sample chunks
                let mut speech_chunks = 0;
                let mut silence_chunks = 0;
                let mut probabilities = Vec::new();

                for (i, chunk) in samples.chunks(512).enumerate() {
                    if chunk.len() == 512 {
                        let probability = vad.process_chunk(chunk).unwrap();
                        probabilities.push(probability);

                        if probability >= 0.5 {
                            speech_chunks += 1;
                        } else {
                            silence_chunks += 1;
                        }

                        if i % 100 == 0 {
                            println!("Chunk {}: probability = {:.3}", i, probability);
                        }
                    }
                }

                let total_chunks = speech_chunks + silence_chunks;
                let speech_percentage = (speech_chunks as f32 / total_chunks as f32) * 100.0;

                println!("\n=== VAD Results ===");
                println!("Total chunks: {}", total_chunks);
                println!("Speech chunks: {} ({:.1}%)", speech_chunks, speech_percentage);
                println!("Silence chunks: {} ({:.1}%)", silence_chunks, 100.0 - speech_percentage);

                if !probabilities.is_empty() {
                    let avg_prob: f32 =
                        probabilities.iter().sum::<f32>() / probabilities.len() as f32;
                    let max_prob = probabilities.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let min_prob = probabilities.iter().cloned().fold(f32::INFINITY, f32::min);

                    println!("Probability range: {:.3} - {:.3}", min_prob, max_prob);
                    println!("Average probability: {:.3}", avg_prob);
                }
            },
            Err(e) => {
                println!("Could not load audio file: {}", e);
                println!("To test with real audio:");
                println!("  1. Convert your audio to 16kHz mono raw f32: ");
                println!("     ffmpeg -i input.ogg -ar 16000 -ac 1 -f f32le output.raw");
                println!("  2. Set TEST_AUDIO_PATH environment variable:");
                println!("     TEST_AUDIO_PATH=output.raw cargo test test_vad_with_real_audio -- --ignored --nocapture");
            },
        }
    }
}
