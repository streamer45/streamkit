// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use pocket_tts::TTSModel;
use streamkit_plugin_sdk_native::prelude::Logger;
use streamkit_plugin_sdk_native::{plugin_info, plugin_warn};

use crate::config::PocketTtsConfig;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ModelCacheKey {
    pub variant: String,
    pub quantized: bool,
    pub config_path: Option<String>,
    pub weights_path: Option<String>,
    pub tokenizer_path: Option<String>,
}

impl ModelCacheKey {
    pub fn from_config(config: &PocketTtsConfig) -> Self {
        Self {
            variant: config.variant.clone(),
            quantized: config.quantized,
            config_path: config.config_path.as_deref().map(normalize_path),
            weights_path: config.weights_path.as_deref().map(normalize_path),
            tokenizer_path: config.tokenizer_path.as_deref().map(normalize_path),
        }
    }
}

static MODEL_CACHE: LazyLock<Mutex<HashMap<ModelCacheKey, Arc<TTSModel>>>> = LazyLock::new(|| {
    tracing::info!("[Pocket TTS Plugin] Initializing model cache");
    Mutex::new(HashMap::new())
});

pub fn get_or_load_model(
    key: &ModelCacheKey,
    config: &PocketTtsConfig,
    logger: &Logger,
) -> Result<Arc<TTSModel>, String> {
    {
        let cache = MODEL_CACHE.lock().map_err(|e| format!("Failed to lock model cache: {e}"))?;
        if let Some(model) = cache.get(key) {
            plugin_info!(
                logger,
                "Pocket TTS model cache hit (variant={}, quantized={})",
                key.variant,
                key.quantized
            );
            return Ok(Arc::clone(model));
        }
    }

    plugin_info!(
        logger,
        "Loading Pocket TTS model (variant={}, quantized={}, offline={})",
        key.variant,
        key.quantized,
        config.weights_path.is_some()
            || config.tokenizer_path.is_some()
            || config.config_path.is_some()
    );

    let model = if config.weights_path.is_some()
        || config.tokenizer_path.is_some()
        || config.config_path.is_some()
    {
        load_model_from_files(config, logger)?
    } else if key.quantized {
        #[cfg(feature = "quantized")]
        {
            TTSModel::load_quantized(&key.variant).map_err(|e| e.to_string())?
        }
        #[cfg(not(feature = "quantized"))]
        {
            return Err(
                "Quantized model requested but plugin was built without feature \"quantized\""
                    .to_string(),
            );
        }
    } else {
        TTSModel::load(&key.variant).map_err(|e| e.to_string())?
    };

    let model = Arc::new(model);

    let mut cache = MODEL_CACHE.lock().map_err(|e| format!("Failed to lock model cache: {e}"))?;
    if let Some(existing) = cache.get(key) {
        return Ok(Arc::clone(existing));
    }
    cache.insert(key.clone(), Arc::clone(&model));
    drop(cache);

    plugin_info!(
        logger,
        "Pocket TTS model loaded and cached (variant={}, quantized={})",
        key.variant,
        key.quantized
    );

    Ok(model)
}

const DEFAULT_VARIANT: &str = "b6369a24";
const DEFAULT_CONFIG_YAML: &str = include_str!("../config/b6369a24.yaml");

fn normalize_path(raw: &str) -> String {
    let path = PathBuf::from(raw);
    path.canonicalize().unwrap_or(path).to_string_lossy().to_string()
}

fn load_model_from_files(config: &PocketTtsConfig, logger: &Logger) -> Result<TTSModel, String> {
    if config.quantized {
        return Err("Quantized mode is not supported with local weights/tokenizer".to_string());
    }

    let weights_path = config
        .weights_path
        .as_ref()
        .ok_or_else(|| "weights_path is required for offline loading".to_string())?;
    let tokenizer_path = config
        .tokenizer_path
        .as_ref()
        .ok_or_else(|| "tokenizer_path is required for offline loading".to_string())?;

    let config_yaml = if let Some(path) = &config.config_path {
        std::fs::read(path).map_err(|e| format!("Failed to read config_path {path}: {e}"))?
    } else if config.variant == DEFAULT_VARIANT {
        DEFAULT_CONFIG_YAML.as_bytes().to_vec()
    } else {
        return Err(format!(
            "config_path is required for variant '{}' when using local weights",
            config.variant
        ));
    };

    let weights_bytes = std::fs::read(weights_path)
        .map_err(|e| format!("Failed to read weights_path {weights_path}: {e}"))?;
    let tokenizer_bytes = std::fs::read(tokenizer_path)
        .map_err(|e| format!("Failed to read tokenizer_path {tokenizer_path}: {e}"))?;

    plugin_info!(
        logger,
        "Loading Pocket TTS model from local files (weights={}, tokenizer={})",
        weights_path,
        tokenizer_path
    );

    TTSModel::load_from_bytes(&config_yaml, &weights_bytes, &tokenizer_bytes)
        .map_err(|e| e.to_string())
}

pub fn configure_model(model: &mut TTSModel, config: &PocketTtsConfig, logger: &Logger) {
    model.temp = config.temperature;
    model.lsd_decode_steps = config.lsd_decode_steps;
    model.eos_threshold = config.eos_threshold;
    model.noise_clamp = config.noise_clamp;
    model.flow_lm.noise_clamp = config.noise_clamp;

    if config.temperature <= 0.0 {
        plugin_warn!(logger, "Pocket TTS temperature <= 0.0 may result in invalid sampling");
    }
}
