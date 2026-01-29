// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use streamkit_plugin_sdk_native::prelude::*;
use supertonic_vendor::TextToSpeech;

/// Wrapper around `TextToSpeech` to mark it as thread-safe.
///
/// # Safety
/// Access to the inner `TextToSpeech` is serialised through the `Mutex` in
/// `CachedModel`, so concurrent mutation cannot occur.
pub struct TtsModelWrapper {
    inner: Mutex<TextToSpeech>,
}

impl TtsModelWrapper {
    pub const fn new(tts: TextToSpeech) -> Self {
        Self { inner: Mutex::new(tts) }
    }

    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, TextToSpeech>, String> {
        self.inner.lock().map_err(|e| format!("Failed to lock TTS model: {e}"))
    }
}

// SAFETY: The inner TextToSpeech is protected by a Mutex.
unsafe impl Send for TtsModelWrapper {}
unsafe impl Sync for TtsModelWrapper {}

struct CachedModel {
    model: Arc<TtsModelWrapper>,
    sample_rate: i32,
}

/// Global cache of loaded TTS models, keyed by canonicalized model_dir.
static MODEL_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedModel>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Load or retrieve a cached TTS model.
///
/// Returns `(Arc<TtsModelWrapper>, sample_rate)`.
pub fn get_or_load_model(
    model_dir: &str,
    logger: &Logger,
) -> Result<(Arc<TtsModelWrapper>, i32), String> {
    {
        let cache = MODEL_CACHE.lock().map_err(|e| format!("Failed to lock model cache: {e}"))?;

        if let Some(cached) = cache.get(model_dir) {
            plugin_info!(
                logger,
                model_dir = %model_dir,
                "CACHE HIT: Reusing cached Supertonic TTS model"
            );
            return Ok((cached.model.clone(), cached.sample_rate));
        }
    }

    plugin_warn!(
        logger,
        model_dir = %model_dir,
        "CACHE MISS: Loading Supertonic TTS model"
    );

    // The upstream HF repo stores ONNX files under an `onnx/` subdirectory.
    // Try `{model_dir}/onnx` first, fall back to `model_dir` directly.
    let onnx_dir = {
        let sub = format!("{model_dir}/onnx");
        if std::path::Path::new(&sub).join("tts.json").exists() {
            sub
        } else {
            model_dir.to_string()
        }
    };

    let tts = supertonic_vendor::load_text_to_speech(&onnx_dir, false)
        .map_err(|e| format!("Failed to load Supertonic model from '{onnx_dir}': {e}"))?;

    let sample_rate = tts.sample_rate;
    let wrapper = Arc::new(TtsModelWrapper::new(tts));

    let cache_size = {
        let mut cache =
            MODEL_CACHE.lock().map_err(|e| format!("Failed to lock model cache: {e}"))?;
        cache.insert(model_dir.to_string(), CachedModel { model: wrapper.clone(), sample_rate });
        cache.len()
    };

    plugin_info!(
        logger,
        model_dir = %model_dir,
        sample_rate = sample_rate,
        cache_size = cache_size,
        "Supertonic TTS model loaded and cached"
    );

    Ok((wrapper, sample_rate))
}
