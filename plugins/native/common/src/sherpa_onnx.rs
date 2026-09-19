// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Manual FFI bindings to the sherpa-onnx C API.
//!
//! Also provides an owning builder for offline TTS engine configs, shared by
//! the sherpa-onnx TTS plugins. Struct layouts mirror
//! https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/c-api/c-api.h

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int};
use std::path::Path;
use std::ptr;

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

impl Default for SherpaOnnxOfflineTtsKokoroModelConfig {
    /// All-null placeholder for when the Kokoro family is not in use.
    fn default() -> Self {
        Self {
            model: ptr::null(),
            voices: ptr::null(),
            tokens: ptr::null(),
            data_dir: ptr::null(),
            length_scale: 1.0,
            dict_dir: ptr::null(),
            lexicon: ptr::null(),
            lang: ptr::null(),
        }
    }
}

/// VITS model configuration
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

impl Default for SherpaOnnxOfflineTtsVitsModelConfig {
    /// All-null placeholder for when the VITS family is not in use.
    fn default() -> Self {
        Self {
            model: ptr::null(),
            lexicon: ptr::null(),
            tokens: ptr::null(),
            data_dir: ptr::null(),
            noise_scale: 0.0,
            noise_scale_w: 0.0,
            length_scale: 1.0,
            dict_dir: ptr::null(),
        }
    }
}

/// Matcha model configuration
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

impl Default for SherpaOnnxOfflineTtsMatchaModelConfig {
    /// All-null placeholder for when the Matcha family is not in use.
    fn default() -> Self {
        Self {
            acoustic_model: ptr::null(),
            vocoder: ptr::null(),
            lexicon: ptr::null(),
            tokens: ptr::null(),
            data_dir: ptr::null(),
            noise_scale: 0.0,
            length_scale: 1.0,
            dict_dir: ptr::null(),
        }
    }
}

/// Kitten model configuration (not used, but needed for struct layout)
#[repr(C)]
pub struct SherpaOnnxOfflineTtsKittenModelConfig {
    pub model: *const c_char,
    pub voices: *const c_char,
    pub tokens: *const c_char,
    pub data_dir: *const c_char,
    pub length_scale: c_float,
}

impl Default for SherpaOnnxOfflineTtsKittenModelConfig {
    /// All-null placeholder for when the Kitten family is not in use.
    fn default() -> Self {
        Self {
            model: ptr::null(),
            voices: ptr::null(),
            tokens: ptr::null(),
            data_dir: ptr::null(),
            length_scale: 1.0,
        }
    }
}

/// Zipvoice model configuration (not used, but needed for struct layout)
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

impl Default for SherpaOnnxOfflineTtsZipvoiceModelConfig {
    /// All-null placeholder for when the Zipvoice family is not in use.
    fn default() -> Self {
        Self {
            tokens: ptr::null(),
            text_model: ptr::null(),
            flow_matching_model: ptr::null(),
            vocoder: ptr::null(),
            data_dir: ptr::null(),
            pinyin_dict: ptr::null(),
            feat_scale: 0.0,
            t_shift: 0.0,
            target_rms: 0.0,
            guidance_scale: 0.0,
        }
    }
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

/// # Errors
/// Returns `Err` if the path contains an interior NUL byte.
pub fn path_to_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| format!("Invalid path: {e}"))
}

fn cstr_or_null(s: Option<&CString>) -> *const c_char {
    s.map_or(ptr::null(), |s| s.as_ptr())
}

/// Owned parameters for a VITS (Piper) model.
///
/// `Option` fields map to a null pointer when `None`.
pub struct VitsParams {
    pub model: CString,
    pub lexicon: Option<CString>,
    pub tokens: CString,
    pub data_dir: CString,
    pub noise_scale: c_float,
    pub noise_scale_w: c_float,
    pub length_scale: c_float,
    pub dict_dir: Option<CString>,
}

impl VitsParams {
    fn as_raw(&self) -> SherpaOnnxOfflineTtsVitsModelConfig {
        SherpaOnnxOfflineTtsVitsModelConfig {
            model: self.model.as_ptr(),
            lexicon: cstr_or_null(self.lexicon.as_ref()),
            tokens: self.tokens.as_ptr(),
            data_dir: self.data_dir.as_ptr(),
            noise_scale: self.noise_scale,
            noise_scale_w: self.noise_scale_w,
            length_scale: self.length_scale,
            dict_dir: cstr_or_null(self.dict_dir.as_ref()),
        }
    }
}

/// Owned parameters for a Matcha model.
pub struct MatchaParams {
    pub acoustic_model: CString,
    pub vocoder: CString,
    pub lexicon: CString,
    pub tokens: CString,
    pub data_dir: CString,
    pub noise_scale: c_float,
    pub length_scale: c_float,
    pub dict_dir: CString,
}

impl MatchaParams {
    fn as_raw(&self) -> SherpaOnnxOfflineTtsMatchaModelConfig {
        SherpaOnnxOfflineTtsMatchaModelConfig {
            acoustic_model: self.acoustic_model.as_ptr(),
            vocoder: self.vocoder.as_ptr(),
            lexicon: self.lexicon.as_ptr(),
            tokens: self.tokens.as_ptr(),
            data_dir: self.data_dir.as_ptr(),
            noise_scale: self.noise_scale,
            length_scale: self.length_scale,
            dict_dir: self.dict_dir.as_ptr(),
        }
    }
}

/// Owned parameters for a Kokoro model.
pub struct KokoroParams {
    pub model: CString,
    pub voices: CString,
    pub tokens: CString,
    pub data_dir: CString,
    pub length_scale: c_float,
    pub dict_dir: CString,
    pub lexicon: CString,
    pub lang: CString,
}

impl KokoroParams {
    fn as_raw(&self) -> SherpaOnnxOfflineTtsKokoroModelConfig {
        SherpaOnnxOfflineTtsKokoroModelConfig {
            model: self.model.as_ptr(),
            voices: self.voices.as_ptr(),
            tokens: self.tokens.as_ptr(),
            data_dir: self.data_dir.as_ptr(),
            length_scale: self.length_scale,
            dict_dir: self.dict_dir.as_ptr(),
            lexicon: self.lexicon.as_ptr(),
            lang: self.lang.as_ptr(),
        }
    }
}

enum ModelParams {
    Vits(VitsParams),
    Matcha(MatchaParams),
    Kokoro(KokoroParams),
}

/// Owned offline-TTS configuration.
///
/// Keeps every `CString` the C config points at alive for the duration of
/// [`Self::create`], and fills the model families that are not in use with the
/// null placeholders the C API expects. `rule_fsts`/`rule_fars`,
/// `max_num_sentences = 1` and `silence_scale = 1.0` are fixed to the values
/// all plugins use.
pub struct OfflineTtsConfig {
    provider: CString,
    num_threads: c_int,
    debug: c_int,
    rule_fsts: CString,
    rule_fars: CString,
    max_num_sentences: c_int,
    silence_scale: c_float,
    model: ModelParams,
}

impl OfflineTtsConfig {
    fn base(provider: CString, num_threads: c_int, debug: bool, model: ModelParams) -> Self {
        Self {
            provider,
            num_threads,
            debug: c_int::from(debug),
            rule_fsts: CString::default(),
            rule_fars: CString::default(),
            max_num_sentences: 1,
            silence_scale: 1.0,
            model,
        }
    }

    /// `provider` is the ONNX execution provider ("cpu", "cuda", ...).
    pub fn vits(provider: CString, num_threads: c_int, debug: bool, params: VitsParams) -> Self {
        Self::base(provider, num_threads, debug, ModelParams::Vits(params))
    }

    /// `provider` is the ONNX execution provider ("cpu", "cuda", ...).
    pub fn matcha(
        provider: CString,
        num_threads: c_int,
        debug: bool,
        params: MatchaParams,
    ) -> Self {
        Self::base(provider, num_threads, debug, ModelParams::Matcha(params))
    }

    /// `provider` is the ONNX execution provider ("cpu", "cuda", ...).
    pub fn kokoro(
        provider: CString,
        num_threads: c_int,
        debug: bool,
        params: KokoroParams,
    ) -> Self {
        Self::base(provider, num_threads, debug, ModelParams::Kokoro(params))
    }

    /// Create the TTS engine. The returned pointer must eventually be freed
    /// with [`SherpaOnnxDestroyOfflineTts`].
    ///
    /// # Errors
    /// Returns `Err` if `SherpaOnnxCreateOfflineTts` returns a null engine.
    pub fn create(&self) -> Result<*mut SherpaOnnxOfflineTts, String> {
        let model = SherpaOnnxOfflineTtsModelConfig {
            vits: match &self.model {
                ModelParams::Vits(p) => p.as_raw(),
                _ => SherpaOnnxOfflineTtsVitsModelConfig::default(),
            },
            num_threads: self.num_threads,
            debug: self.debug,
            provider: self.provider.as_ptr(),
            matcha: match &self.model {
                ModelParams::Matcha(p) => p.as_raw(),
                _ => SherpaOnnxOfflineTtsMatchaModelConfig::default(),
            },
            kokoro: match &self.model {
                ModelParams::Kokoro(p) => p.as_raw(),
                _ => SherpaOnnxOfflineTtsKokoroModelConfig::default(),
            },
            kitten: SherpaOnnxOfflineTtsKittenModelConfig::default(),
            zipvoice: SherpaOnnxOfflineTtsZipvoiceModelConfig::default(),
        };
        let config = SherpaOnnxOfflineTtsConfig {
            model,
            rule_fsts: self.rule_fsts.as_ptr(),
            max_num_sentences: self.max_num_sentences,
            rule_fars: self.rule_fars.as_ptr(),
            silence_scale: self.silence_scale,
        };

        // SAFETY: `config` only points at `CString`s owned by `self`, which are
        // alive for the whole call; sherpa-onnx copies them while constructing
        // the engine.
        let tts = unsafe { SherpaOnnxCreateOfflineTts(&raw const config) };
        if tts.is_null() {
            return Err("Failed to create TTS engine".to_string());
        }
        Ok(tts)
    }
}
