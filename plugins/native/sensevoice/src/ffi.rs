// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! FFI bindings to sherpa-onnx C API for SenseVoice
//! Based on https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/c-api/c-api.h

use std::os::raw::{c_char, c_float, c_int};

/// Opaque recognizer handle
#[repr(C)]
pub struct SherpaOnnxOfflineRecognizer {
    _private: [u8; 0],
}

/// Opaque stream handle
#[repr(C)]
pub struct SherpaOnnxOfflineStream {
    _private: [u8; 0],
}

/// Recognition result
#[repr(C)]
pub struct SherpaOnnxOfflineRecognizerResult {
    pub text: *const c_char,
    pub tokens: *const c_char,
    pub timestamps: *const c_float,
    pub count: c_int,
    pub lang: *const c_char,
    pub emotion: *const c_char,
    pub event: *const c_char,
    pub json: *const c_char,
}

/// SenseVoice model configuration
#[repr(C)]
pub struct SherpaOnnxOfflineSenseVoiceModelConfig {
    pub model: *const c_char,
    pub language: *const c_char,
    pub use_itn: c_int, // 1 = true, 0 = false
}

/// Transducer model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTransducerModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
    pub joiner: *const c_char,
}

/// Paraformer model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineParaformerModelConfig {
    pub model: *const c_char,
}

/// NeMo CTC model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineNemoEncDecCtcModelConfig {
    pub model: *const c_char,
}

/// Whisper model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineWhisperModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
    pub language: *const c_char,
    pub task: *const c_char,
    pub tail_paddings: c_int,
}

/// TDnn model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTdnnModelConfig {
    pub model: *const c_char,
}

/// Moonshine model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineMoonshineModelConfig {
    pub preprocessor: *const c_char,
    pub encoder: *const c_char,
    pub uncached_decoder: *const c_char,
    pub cached_decoder: *const c_char,
}

/// FireRedAsr model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineFireRedAsrModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
}

/// Dolphin model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineDolphinModelConfig {
    pub model: *const c_char,
}

/// ZipformerCtc model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineZipformerCtcModelConfig {
    pub model: *const c_char,
}

/// Canary model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineCanaryModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
    pub src_lang: *const c_char,
    pub tgt_lang: *const c_char,
    pub use_pnc: c_int,
}

/// WenetCtc model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineWenetCtcModelConfig {
    pub model: *const c_char,
}

/// OmnilingualAsrCtc model config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineOmnilingualAsrCtcModelConfig {
    pub model: *const c_char,
}

/// LM configuration
#[repr(C)]
pub struct SherpaOnnxOfflineLMConfig {
    pub model: *const c_char,
    pub scale: c_float,
}

/// Overall model configuration
#[repr(C)]
pub struct SherpaOnnxOfflineModelConfig {
    pub transducer: SherpaOnnxOfflineTransducerModelConfig,
    pub paraformer: SherpaOnnxOfflineParaformerModelConfig,
    pub nemo_ctc: SherpaOnnxOfflineNemoEncDecCtcModelConfig,
    pub whisper: SherpaOnnxOfflineWhisperModelConfig,
    pub tdnn: SherpaOnnxOfflineTdnnModelConfig,
    pub tokens: *const c_char,
    pub num_threads: c_int,
    pub debug: c_int,
    pub provider: *const c_char,
    pub model_type: *const c_char,
    pub modeling_unit: *const c_char,
    pub bpe_vocab: *const c_char,
    pub telespeech_ctc: *const c_char,
    pub sense_voice: SherpaOnnxOfflineSenseVoiceModelConfig,
    pub moonshine: SherpaOnnxOfflineMoonshineModelConfig,
    pub fire_red_asr: SherpaOnnxOfflineFireRedAsrModelConfig,
    pub dolphin: SherpaOnnxOfflineDolphinModelConfig,
    pub zipformer_ctc: SherpaOnnxOfflineZipformerCtcModelConfig,
    pub canary: SherpaOnnxOfflineCanaryModelConfig,
    pub wenet_ctc: SherpaOnnxOfflineWenetCtcModelConfig,
    pub omnilingual: SherpaOnnxOfflineOmnilingualAsrCtcModelConfig,
}

/// Homophone replacer config (unused, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxHomophoneReplacerConfig {
    pub dict_dir: *const c_char,
    pub lexicon: *const c_char,
    pub rule_fsts: *const c_char,
}

/// Recognizer configuration
#[repr(C)]
pub struct SherpaOnnxOfflineRecognizerConfig {
    pub feat_config: SherpaOnnxFeatureConfig,
    pub model_config: SherpaOnnxOfflineModelConfig,
    pub lm_config: SherpaOnnxOfflineLMConfig,
    pub decoding_method: *const c_char,
    pub max_active_paths: c_int,
    pub hotwords_file: *const c_char,
    pub hotwords_score: c_float,
    pub rule_fsts: *const c_char,
    pub rule_fars: *const c_char,
    pub blank_penalty: c_float,
    pub hr: SherpaOnnxHomophoneReplacerConfig,
}

/// Feature extraction configuration
#[repr(C)]
pub struct SherpaOnnxFeatureConfig {
    pub sample_rate: c_int,
    pub feature_dim: c_int,
}

/// Wave data structure (currently unused but kept for future use)
#[allow(dead_code)]
#[repr(C)]
pub struct SherpaOnnxWave {
    pub samples: *const c_float,
    pub sample_rate: c_int,
    pub num_samples: c_int,
}

extern "C" {
    /// Create offline recognizer
    pub fn SherpaOnnxCreateOfflineRecognizer(
        config: *const SherpaOnnxOfflineRecognizerConfig,
    ) -> *mut SherpaOnnxOfflineRecognizer;

    /// Destroy offline recognizer
    pub fn SherpaOnnxDestroyOfflineRecognizer(recognizer: *mut SherpaOnnxOfflineRecognizer);

    /// Create offline stream
    pub fn SherpaOnnxCreateOfflineStream(
        recognizer: *const SherpaOnnxOfflineRecognizer,
    ) -> *mut SherpaOnnxOfflineStream;

    /// Destroy offline stream
    pub fn SherpaOnnxDestroyOfflineStream(stream: *mut SherpaOnnxOfflineStream);

    /// Accept waveform for offline stream
    pub fn SherpaOnnxAcceptWaveformOffline(
        stream: *mut SherpaOnnxOfflineStream,
        sample_rate: c_int,
        samples: *const c_float,
        n: c_int,
    );

    /// Decode offline stream
    pub fn SherpaOnnxDecodeOfflineStream(
        recognizer: *mut SherpaOnnxOfflineRecognizer,
        stream: *mut SherpaOnnxOfflineStream,
    );

    /// Get recognition result
    pub fn SherpaOnnxGetOfflineStreamResult(
        stream: *const SherpaOnnxOfflineStream,
    ) -> *const SherpaOnnxOfflineRecognizerResult;

    /// Destroy recognition result
    pub fn SherpaOnnxDestroyOfflineRecognizerResult(
        result: *const SherpaOnnxOfflineRecognizerResult,
    );

    /// Read wave file (utility function, currently unused but kept for future use)
    #[allow(dead_code)]
    pub fn SherpaOnnxReadWave(filename: *const c_char) -> *const SherpaOnnxWave;

    /// Free wave data (currently unused but kept for future use)
    #[allow(dead_code)]
    pub fn SherpaOnnxFreeWave(wave: *const SherpaOnnxWave);
}
