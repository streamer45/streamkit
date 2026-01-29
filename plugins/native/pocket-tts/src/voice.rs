// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use base64::{engine::general_purpose, Engine as _};
use pocket_tts::weights::download_if_necessary;
use pocket_tts::{audio, ModelState, TTSModel};
use streamkit_plugin_sdk_native::plugin_info;
use streamkit_plugin_sdk_native::prelude::Logger;

use crate::model::ModelCacheKey;

pub const PREDEFINED_VOICES: &[&str] =
    &["alba", "marius", "javert", "jean", "fantine", "cosette", "eponine", "azelma"];

const STOCK_VOICE_REPO: &str = "kyutai/pocket-tts-without-voice-cloning";
const DEFAULT_VOICE: &str = "alba";

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct VoiceCacheKey {
    pub model_key: ModelCacheKey,
    pub voice_spec: String,
}

static VOICE_CACHE: LazyLock<Mutex<HashMap<VoiceCacheKey, Arc<ModelState>>>> =
    LazyLock::new(|| {
        tracing::info!("[Pocket TTS Plugin] Initializing voice cache");
        Mutex::new(HashMap::new())
    });

pub fn normalize_voice_spec(voice: &str, voice_dir: Option<&str>) -> String {
    let trimmed = voice.trim();
    if trimmed.is_empty() {
        return DEFAULT_VOICE.to_string();
    }

    if PREDEFINED_VOICES.contains(&trimmed) {
        if let Some(path) = resolve_predefined_voice_path(voice_dir, trimmed) {
            return path.to_string_lossy().to_string();
        }
    }

    if is_base64_audio(trimmed) {
        return trimmed.to_string();
    }

    if trimmed.starts_with("hf://") {
        return trimmed.to_string();
    }

    let path = PathBuf::from(trimmed);
    if path.exists() {
        return path.canonicalize().unwrap_or(path).to_string_lossy().to_string();
    }

    trimmed.to_string()
}

pub fn get_or_load_voice_state(
    model: &TTSModel,
    key: &VoiceCacheKey,
    voice_dir: Option<&str>,
    logger: &Logger,
) -> Result<Arc<ModelState>, String> {
    {
        let cache = VOICE_CACHE.lock().map_err(|e| format!("Failed to lock voice cache: {e}"))?;
        if let Some(state) = cache.get(key) {
            plugin_info!(logger, "Pocket TTS voice cache hit (voice={})", key.voice_spec);
            return Ok(Arc::clone(state));
        }
    }

    plugin_info!(logger, "Loading voice state: {}", key.voice_spec);

    let state = resolve_voice_spec(model, &key.voice_spec, voice_dir)?;
    let state = Arc::new(state);

    let mut cache = VOICE_CACHE.lock().map_err(|e| format!("Failed to lock voice cache: {e}"))?;
    if let Some(existing) = cache.get(key) {
        return Ok(Arc::clone(existing));
    }
    cache.insert(key.clone(), Arc::clone(&state));
    drop(cache);

    Ok(state)
}

fn resolve_voice_spec(
    model: &TTSModel,
    spec: &str,
    voice_dir: Option<&str>,
) -> Result<ModelState, String> {
    let spec = spec.trim();

    if spec.is_empty() {
        return resolve_predefined_voice(model, DEFAULT_VOICE, voice_dir);
    }

    if PREDEFINED_VOICES.contains(&spec) {
        return resolve_predefined_voice(model, spec, voice_dir);
    }

    if spec.starts_with("hf://") {
        return resolve_hf_voice(model, spec);
    }

    let path = PathBuf::from(spec);
    if path.exists() {
        return resolve_file_voice(model, &path);
    }

    if is_base64_audio(spec) {
        return resolve_base64_voice(model, spec);
    }

    Err(format!(
        "Voice '{spec}' not found. Expected predefined name, local .wav/.safetensors, hf:// URL, or base64 audio",
    ))
}

fn resolve_predefined_voice(
    model: &TTSModel,
    name: &str,
    voice_dir: Option<&str>,
) -> Result<ModelState, String> {
    if let Some(path) = resolve_predefined_voice_path(voice_dir, name) {
        return model
            .get_voice_state_from_prompt_file(&path)
            .map_err(|e| format!("Failed to load local voice embeddings {}: {e}", path.display()));
    }

    if let Some(dir) = voice_dir {
        return Err(format!("Voice embeddings for '{name}' not found in {dir}"));
    }

    let hf_path = format!("hf://{STOCK_VOICE_REPO}/embeddings/{name}.safetensors");
    let local_path = download_if_necessary(&hf_path)
        .map_err(|e| format!("Failed to download stock voice '{name}': {e}"))?;

    model
        .get_voice_state_from_prompt_file(&local_path)
        .map_err(|e| format!("Failed to load voice embeddings {}: {e}", local_path.display()))
}

fn resolve_predefined_voice_path(voice_dir: Option<&str>, name: &str) -> Option<PathBuf> {
    let dir = voice_dir?;
    let base = PathBuf::from(dir);
    let direct = base.join(format!("{name}.safetensors"));
    if direct.exists() {
        return Some(direct.canonicalize().unwrap_or(direct));
    }
    let nested = base.join("embeddings").join(format!("{name}.safetensors"));
    if nested.exists() {
        return Some(nested.canonicalize().unwrap_or(nested));
    }
    None
}

fn resolve_hf_voice(model: &TTSModel, url: &str) -> Result<ModelState, String> {
    let local_path = download_if_necessary(url)
        .map_err(|e| format!("Failed to download voice from '{url}': {e}"))?;

    resolve_file_voice(model, &local_path)
}

fn resolve_file_voice(model: &TTSModel, path: &PathBuf) -> Result<ModelState, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    match ext.as_str() {
        "safetensors" => model
            .get_voice_state_from_prompt_file(path)
            .map_err(|e| format!("Failed to load embeddings from {}: {e}", path.display())),
        "wav" | "wave" => model
            .get_voice_state(path)
            .map_err(|e| format!("Failed to process voice audio {}: {e}", path.display())),
        _ => {
            Err(format!("Unsupported voice file extension '{ext}'. Expected .wav or .safetensors"))
        },
    }
}

fn resolve_base64_voice(model: &TTSModel, spec: &str) -> Result<ModelState, String> {
    voice_state_from_base64(model, spec)
}

fn is_base64_audio(spec: &str) -> bool {
    if spec.starts_with("data:audio/") && spec.contains("base64,") {
        return true;
    }

    if spec.len() > 100 {
        let clean = spec.trim();
        return clean
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
    }

    false
}

pub fn voice_state_from_base64(model: &TTSModel, spec: &str) -> Result<ModelState, String> {
    let b64_str =
        if spec.starts_with("data:") { spec.split(',').nth(1).unwrap_or(spec) } else { spec };

    let bytes = general_purpose::STANDARD
        .decode(b64_str)
        .map_err(|e| format!("Failed to decode base64 audio: {e}"))?;

    voice_state_from_wav_bytes(model, &bytes)
}

pub fn voice_state_from_wav_bytes(model: &TTSModel, bytes: &[u8]) -> Result<ModelState, String> {
    let (audio, sample_rate) =
        audio::read_wav_from_bytes(bytes).map_err(|e| format!("WAV decode failed: {e}"))?;

    let model_sample_rate = u32::try_from(model.sample_rate)
        .map_err(|_| format!("Model sample rate {} does not fit in u32", model.sample_rate))?;
    let audio = if sample_rate == model_sample_rate {
        audio
    } else {
        audio::resample(&audio, sample_rate, model_sample_rate)
            .map_err(|e| format!("Failed to resample voice audio: {e}"))?
    };

    let audio = audio.unsqueeze(0).map_err(|e| format!("Failed to add batch dimension: {e}"))?;

    model
        .get_voice_state_from_tensor(&audio)
        .map_err(|e| format!("Failed to encode voice audio: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predefined_voices() {
        assert!(PREDEFINED_VOICES.contains(&"alba"));
        assert!(!PREDEFINED_VOICES.contains(&"unknown"));
    }

    #[test]
    fn test_is_base64_audio() {
        assert!(is_base64_audio(
            "data:audio/wav;base64,UklGRi4AAABXQVZFZm10IBAAAAABAAIAQB8AAEAfAAABAAgAZGF0YQoAAAAA"
        ));
        assert!(!is_base64_audio("alba"));
        assert!(!is_base64_audio("/path/to/file.wav"));
    }
}
