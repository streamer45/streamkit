// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Manual FFI bindings to Sherpa-ONNX C API
//! Based on https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/c-api/c-api.h

use std::os::raw::{c_char, c_float, c_int, c_void};

/// Opaque TTS engine handle
#[repr(C)]
pub struct SherpaOnnxOfflineTts {
    _private: [u8; 0],
}

/// Generated audio data
#[repr(C)]
pub struct SherpaOnnxOfflineTtsGeneratedAudio {
    pub samples: *const c_float,
    pub n: c_int,
    pub sample_rate: c_int,
}

/// Kokoro model configuration
#[repr(C)]
pub struct SherpaOnnxOfflineTtsKokoroModelConfig {
    pub model: *const c_char,
    pub voices: *const c_char,
    pub tokens: *const c_char,
    pub data_dir: *const c_char,
    pub length_scale: c_float,
    pub dict_dir: *const c_char,
    pub lexicon: *const c_char,
    pub lang: *const c_char,
}

/// VITS model configuration (unused but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTtsVitsModelConfig {
    pub model: *const c_char,
    pub lexicon: *const c_char,
    pub tokens: *const c_char,
    pub data_dir: *const c_char,
    pub noise_scale: c_float,
    pub noise_scale_w: c_float,
    pub length_scale: c_float,
    pub dict_dir: *const c_char,
}

/// Placeholder for Matcha model config (not used, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTtsMatchaModelConfig {
    pub acoustic_model: *const c_char,
    pub vocoder: *const c_char,
    pub lexicon: *const c_char,
    pub tokens: *const c_char,
    pub data_dir: *const c_char,
    pub noise_scale: c_float,
    pub length_scale: c_float,
    pub dict_dir: *const c_char,
}

/// Placeholder for Kitten model config (not used, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTtsKittenModelConfig {
    pub model: *const c_char,
    pub voices: *const c_char,
    pub tokens: *const c_char,
    pub data_dir: *const c_char,
    pub length_scale: c_float,
}

/// Placeholder for Zipvoice model config (not used, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTtsZipvoiceModelConfig {
    pub tokens: *const c_char,
    pub text_model: *const c_char,
    pub flow_matching_model: *const c_char,
    pub vocoder: *const c_char,
    pub data_dir: *const c_char,
    pub pinyin_dict: *const c_char,
    pub feat_scale: c_float,
    pub t_shift: c_float,
    pub target_rms: c_float,
    pub guidance_scale: c_float,
}

/// TTS model configuration
#[repr(C)]
pub struct SherpaOnnxOfflineTtsModelConfig {
    pub vits: SherpaOnnxOfflineTtsVitsModelConfig,
    pub num_threads: c_int,
    pub debug: c_int,
    pub provider: *const c_char,
    pub matcha: SherpaOnnxOfflineTtsMatchaModelConfig,
    pub kokoro: SherpaOnnxOfflineTtsKokoroModelConfig,
    pub kitten: SherpaOnnxOfflineTtsKittenModelConfig,
    pub zipvoice: SherpaOnnxOfflineTtsZipvoiceModelConfig,
}

/// TTS configuration
#[repr(C)]
pub struct SherpaOnnxOfflineTtsConfig {
    pub model: SherpaOnnxOfflineTtsModelConfig,
    pub rule_fsts: *const c_char,
    pub max_num_sentences: c_int,
    pub rule_fars: *const c_char,
    pub silence_scale: c_float,
}

/// Callback function type: (samples, count, arg) -> continue (1) or stop (0)
// Allow: FFI type alias for future use (streaming audio generation)
#[allow(dead_code)]
pub type SherpaOnnxGeneratedAudioCallbackWithArg =
    Option<extern "C" fn(samples: *const c_float, n: c_int, arg: *mut c_void) -> c_int>;

extern "C" {
    /// Create TTS engine
    pub fn SherpaOnnxCreateOfflineTts(
        config: *const SherpaOnnxOfflineTtsConfig,
    ) -> *mut SherpaOnnxOfflineTts;

    /// Destroy TTS engine
    pub fn SherpaOnnxDestroyOfflineTts(tts: *mut SherpaOnnxOfflineTts);

    /// Generate audio (non-callback, faster)
    pub fn SherpaOnnxOfflineTtsGenerate(
        tts: *const SherpaOnnxOfflineTts,
        text: *const c_char,
        sid: c_int,
        speed: c_float,
    ) -> *const SherpaOnnxOfflineTtsGeneratedAudio;

    /// Destroy generated audio
    pub fn SherpaOnnxDestroyOfflineTtsGeneratedAudio(
        audio: *const SherpaOnnxOfflineTtsGeneratedAudio,
    );
}
