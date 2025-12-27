// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! FFI bindings to sherpa-onnx C API for VAD (Voice Activity Detection)
//! Based on https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/c-api/c-api.h

use std::os::raw::{c_char, c_float, c_int};

/// Opaque voice activity detector handle
#[repr(C)]
pub struct SherpaOnnxVoiceActivityDetector {
    _private: [u8; 0],
}

/// Detected speech segment
#[repr(C)]
pub struct SherpaOnnxSpeechSegment {
    /// Start index in the buffer
    pub start: c_int,
    /// Pointer to the samples (f32 array)
    pub samples: *const c_float,
    /// Number of samples in the segment
    pub n: c_int,
}

/// Silero VAD model configuration (unused but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxSileroVadModelConfig {
    pub model: *const c_char,
    pub threshold: c_float,
    pub min_silence_duration: c_float,
    pub min_speech_duration: c_float,
    pub window_size: c_int,
    pub max_speech_duration: c_float,
}

/// Ten-VAD model configuration
#[repr(C)]
pub struct SherpaOnnxTenVadModelConfig {
    pub model: *const c_char,
    pub threshold: c_float,
    pub min_silence_duration: c_float,
    pub min_speech_duration: c_float,
    pub window_size: c_int,
    pub max_speech_duration: c_float,
}

/// Overall VAD model configuration
#[repr(C)]
pub struct SherpaOnnxVadModelConfig {
    pub silero_vad: SherpaOnnxSileroVadModelConfig,
    pub sample_rate: c_int,
    pub num_threads: c_int,
    pub provider: *const c_char,
    pub debug: c_int,
    pub ten_vad: SherpaOnnxTenVadModelConfig,
}

extern "C" {
    /// Create voice activity detector
    pub fn SherpaOnnxCreateVoiceActivityDetector(
        config: *const SherpaOnnxVadModelConfig,
        buffer_size_in_seconds: c_float,
    ) -> *mut SherpaOnnxVoiceActivityDetector;

    /// Destroy voice activity detector
    pub fn SherpaOnnxDestroyVoiceActivityDetector(vad: *mut SherpaOnnxVoiceActivityDetector);

    /// Accept audio waveform
    pub fn SherpaOnnxVoiceActivityDetectorAcceptWaveform(
        vad: *mut SherpaOnnxVoiceActivityDetector,
        samples: *const c_float,
        n: c_int,
    );

    /// Check if voice/speech is detected
    /// Returns 1 if detected, 0 otherwise
    pub fn SherpaOnnxVoiceActivityDetectorDetected(
        vad: *const SherpaOnnxVoiceActivityDetector,
    ) -> c_int;

    /// Check if the segment queue is empty
    /// Returns 1 if empty, 0 otherwise
    pub fn SherpaOnnxVoiceActivityDetectorEmpty(
        vad: *const SherpaOnnxVoiceActivityDetector,
    ) -> c_int;

    /// Get the first detected speech segment
    pub fn SherpaOnnxVoiceActivityDetectorFront(
        vad: *const SherpaOnnxVoiceActivityDetector,
    ) -> *const SherpaOnnxSpeechSegment;

    /// Remove the first detected speech segment from the queue
    pub fn SherpaOnnxVoiceActivityDetectorPop(vad: *mut SherpaOnnxVoiceActivityDetector);

    /// Clear all detected speech segments (currently unused but part of the API)
    #[allow(dead_code)]
    pub fn SherpaOnnxVoiceActivityDetectorClear(vad: *mut SherpaOnnxVoiceActivityDetector);

    /// Reset the detector state
    pub fn SherpaOnnxVoiceActivityDetectorReset(vad: *mut SherpaOnnxVoiceActivityDetector);

    /// Flush pending audio data
    pub fn SherpaOnnxVoiceActivityDetectorFlush(vad: *mut SherpaOnnxVoiceActivityDetector);

    /// Destroy a speech segment
    pub fn SherpaOnnxDestroySpeechSegment(segment: *const SherpaOnnxSpeechSegment);
}
