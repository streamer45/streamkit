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
    #[allow(dead_code)]
    pub const fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.0, 1.0);
    }

    /// Get current threshold
    #[allow(dead_code)]
    pub const fn threshold(&self) -> f32 {
        self.threshold
    }
}
