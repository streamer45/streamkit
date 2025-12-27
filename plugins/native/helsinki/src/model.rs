// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Model loading and caching for Helsinki-NLP OPUS-MT translation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use candle_core::{DType, Device};
use candle_nn::VarBuilder;
use candle_transformers::models::marian::{Config, MTModel};
use candle_nn::Activation;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::{plugin_info, plugin_warn};
use tokenizers::Tokenizer;

use crate::config::HelsinkiConfig;

/// HuggingFace/Transformers config.json format for Marian models.
/// Maps to Candle's Config struct with proper type conversions.
#[derive(Debug, Deserialize)]
struct HfMarianConfig {
    vocab_size: usize,
    decoder_vocab_size: Option<usize>,
    #[serde(default = "default_max_position_embeddings")]
    max_position_embeddings: usize,
    encoder_layers: usize,
    encoder_ffn_dim: usize,
    encoder_attention_heads: usize,
    decoder_layers: usize,
    decoder_ffn_dim: usize,
    decoder_attention_heads: usize,
    #[serde(default = "default_true")]
    use_cache: bool,
    #[serde(default = "default_true")]
    is_encoder_decoder: bool,
    #[serde(default = "default_activation")]
    activation_function: String,
    d_model: usize,
    decoder_start_token_id: u32,
    #[serde(default = "default_true")]
    scale_embedding: bool,
    pad_token_id: u32,
    eos_token_id: u32,
    #[serde(default)]
    forced_eos_token_id: u32,
    #[serde(default = "default_true")]
    share_encoder_decoder_embeddings: bool,
}

fn default_max_position_embeddings() -> usize { 512 }
fn default_true() -> bool { true }
fn default_activation() -> String { "gelu".to_string() }

impl HfMarianConfig {
    /// Convert to Candle's Config struct.
    fn to_candle_config(&self) -> Config {
        let activation = match self.activation_function.as_str() {
            "swish" | "silu" => Activation::Silu,
            "gelu" | "gelu_new" => Activation::Gelu,
            "relu" => Activation::Relu,
            _ => Activation::Gelu, // Default to GELU for unknown activations
        };

        Config {
            vocab_size: self.vocab_size,
            decoder_vocab_size: self.decoder_vocab_size,
            max_position_embeddings: self.max_position_embeddings,
            encoder_layers: self.encoder_layers,
            encoder_ffn_dim: self.encoder_ffn_dim,
            encoder_attention_heads: self.encoder_attention_heads,
            decoder_layers: self.decoder_layers,
            decoder_ffn_dim: self.decoder_ffn_dim,
            decoder_attention_heads: self.decoder_attention_heads,
            use_cache: self.use_cache,
            is_encoder_decoder: self.is_encoder_decoder,
            activation_function: activation,
            d_model: self.d_model,
            decoder_start_token_id: self.decoder_start_token_id,
            scale_embedding: self.scale_embedding,
            pad_token_id: self.pad_token_id,
            eos_token_id: self.eos_token_id,
            forced_eos_token_id: self.forced_eos_token_id,
            share_encoder_decoder_embeddings: self.share_encoder_decoder_embeddings,
        }
    }
}

/// Cache key: (model_dir, device, device_index)
type TranslatorCacheKey = (String, String, usize);

/// Cached translator containing model and tokenizers.
pub struct CachedTranslator {
    /// The Marian MT model.
    pub model: MTModel,
    /// Tokenizer for encoding source text.
    pub source_tokenizer: Tokenizer,
    /// Tokenizer for decoding target text.
    pub target_tokenizer: Tokenizer,
    /// Model configuration.
    pub config: Config,
    /// Device the model is loaded on.
    pub device: Device,
}

// Safety: CachedTranslator is Send because all fields are Send
unsafe impl Send for CachedTranslator {}

/// Wrapper for thread-safe access to cached translator.
struct CachedTranslatorEntry {
    translator: Arc<Mutex<CachedTranslator>>,
}

/// Global cache of translators.
static TRANSLATOR_CACHE: LazyLock<Mutex<HashMap<TranslatorCacheKey, CachedTranslatorEntry>>> =
    LazyLock::new(|| {
        tracing::info!("[Helsinki Plugin] Initializing translator cache");
        Mutex::new(HashMap::new())
    });

/// GPU availability status: 0 = not checked, 1 = available, 2 = not available
#[cfg(feature = "cuda")]
static GPU_AVAILABILITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Check if GPU/CUDA is available.
#[cfg(feature = "cuda")]
fn is_gpu_available() -> bool {
    use std::sync::atomic::Ordering;

    let status = GPU_AVAILABILITY.load(Ordering::Relaxed);
    if status != 0 {
        return status == 1;
    }

    let available = check_cuda_available();
    GPU_AVAILABILITY.store(if available { 1 } else { 2 }, Ordering::Relaxed);
    tracing::info!(
        "[Helsinki Plugin] GPU availability check: available={}",
        available
    );
    available
}

/// Check for CUDA availability by looking for device files and libraries.
#[cfg(feature = "cuda")]
fn check_cuda_available() -> bool {
    // Check for NVIDIA device file
    if Path::new("/dev/nvidia0").exists() {
        return true;
    }

    // Check for CUDA environment variables
    if std::env::var("CUDA_VISIBLE_DEVICES").is_ok()
        || std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok()
    {
        return true;
    }

    // Check for CUDA libraries
    let cuda_paths = [
        "/usr/local/cuda/lib64/libcudart.so",
        "/usr/lib/x86_64-linux-gnu/libcuda.so",
        "/usr/lib64/libcuda.so",
    ];

    cuda_paths.iter().any(|p| Path::new(p).exists())
}

/// Get the Candle device based on configuration.
pub fn get_device(config: &HelsinkiConfig) -> Result<Device, String> {
    match config.normalized_device().as_str() {
        "cpu" => Ok(Device::Cpu),
        "cuda" => {
            #[cfg(feature = "cuda")]
            {
                Device::new_cuda(config.device_index)
                    .map_err(|e| format!("CUDA device {} not available: {}", config.device_index, e))
            }
            #[cfg(not(feature = "cuda"))]
            {
                Err("CUDA support not compiled in. Rebuild with --features cuda".to_string())
            }
        }
        "auto" => {
            #[cfg(feature = "cuda")]
            {
                if is_gpu_available() {
                    Device::new_cuda(config.device_index).or_else(|_| Ok(Device::Cpu))
                } else {
                    Ok(Device::Cpu)
                }
            }
            #[cfg(not(feature = "cuda"))]
            {
                Ok(Device::Cpu)
            }
        }
        other => Err(format!(
            "Invalid device '{}'. Use 'cpu', 'cuda', or 'auto'",
            other
        )),
    }
}

/// Load model configuration from JSON file or use preset.
fn load_config(model_dir: &str, source_lang: &str, target_lang: &str) -> Result<Config, String> {
    let config_path = Path::new(model_dir).join("config.json");

    if config_path.exists() {
        // Load from file and convert to Candle format
        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config.json: {}", e))?;
        let hf_config: HfMarianConfig = serde_json::from_str(&config_str)
            .map_err(|e| format!("Failed to parse config.json: {}", e))?;

        tracing::info!(
            "[Helsinki Plugin] Loaded config from file: vocab_size={}, d_model={}, encoder_layers={}, decoder_layers={}",
            hf_config.vocab_size,
            hf_config.d_model,
            hf_config.encoder_layers,
            hf_config.decoder_layers
        );

        Ok(hf_config.to_candle_config())
    } else {
        // Use preset based on language pair
        tracing::warn!(
            "[Helsinki Plugin] config.json not found in {}, using preset for {}-{}",
            model_dir, source_lang, target_lang
        );
        match (source_lang, target_lang) {
            ("en", "es") => Ok(Config::opus_mt_en_es()),
            ("en", "fr") => Ok(Config::opus_mt_en_fr()),
            ("en", "ru") => Ok(Config::opus_mt_en_ru()),
            ("en", "zh") => Ok(Config::opus_mt_en_zh()),
            ("en", "hi") => Ok(Config::opus_mt_en_hi()),
            ("fr", "en") => Ok(Config::opus_mt_fr_en()),
            _ => Err(format!(
                "No preset config for {}->{} and config.json not found in {}",
                source_lang, target_lang, model_dir
            )),
        }
    }
}

/// Load model weights from safetensors file.
fn load_weights(model_dir: &str, device: &Device) -> Result<VarBuilder<'static>, String> {
    let model_path = Path::new(model_dir).join("model.safetensors");

    if !model_path.exists() {
        return Err(format!(
            "Model file not found: {}. Download with: just download-helsinki-models",
            model_path.display()
        ));
    }

    // Load safetensors with f32 dtype
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[model_path], DType::F32, device)
            .map_err(|e| format!("Failed to load model weights: {}", e))?
    };

    Ok(vb)
}

/// Load tokenizer from JSON file.
fn load_tokenizers(model_dir: &str) -> Result<(Tokenizer, Tokenizer), String> {
    let source_path = Path::new(model_dir).join("source_tokenizer.json");
    let target_path = Path::new(model_dir).join("target_tokenizer.json");
    let shared_path = Path::new(model_dir).join("tokenizer.json");

    if source_path.exists() && target_path.exists() {
        validate_tokenizer_json(&source_path)?;
        validate_tokenizer_json(&target_path)?;
        let source = Tokenizer::from_file(&source_path)
            .map_err(|e| format!("Failed to load source_tokenizer.json: {}", e))?;
        let target = Tokenizer::from_file(&target_path)
            .map_err(|e| format!("Failed to load target_tokenizer.json: {}", e))?;
        return Ok((source, target));
    }

    if shared_path.exists() {
        validate_tokenizer_json(&shared_path)?;
        let source = Tokenizer::from_file(&shared_path)
            .map_err(|e| format!("Failed to load tokenizer.json: {}", e))?;
        let target = Tokenizer::from_file(&shared_path)
            .map_err(|e| format!("Failed to load tokenizer.json: {}", e))?;
        return Ok((source, target));
    }

    Err(format!(
        "Tokenizers not found in {}. Expected source_tokenizer.json + target_tokenizer.json (preferred) or tokenizer.json. Re-run: just download-helsinki-models",
        model_dir
    ))
}

/// Get or load a cached translator.
pub fn get_or_load_translator(
    config: &HelsinkiConfig,
    logger: &Logger,
) -> Result<Arc<Mutex<CachedTranslator>>, String> {
    let cache_key = (
        config.model_dir.clone(),
        config.normalized_device(),
        config.device_index,
    );

    // Check cache first
    {
        let cache = TRANSLATOR_CACHE
            .lock()
            .map_err(|e| format!("Cache lock failed: {}", e))?;

        if let Some(entry) = cache.get(&cache_key) {
            plugin_info!(logger, "CACHE HIT: Reusing Helsinki translator");
            return Ok(entry.translator.clone());
        }
    }

    plugin_warn!(
        logger,
        "CACHE MISS: Loading Helsinki model from {}",
        config.model_dir
    );

    // Load model configuration
    let model_config = load_config(
        &config.model_dir,
        &config.source_language,
        &config.target_language,
    )?;

    // Initialize device
    let device = get_device(config)?;
    plugin_info!(logger, "Using device: {:?}", device);

    // Load model weights
    let vb = load_weights(&config.model_dir, &device)?;

    // Create model
    let model = MTModel::new(&model_config, vb)
        .map_err(|e| format!("Failed to create MTModel: {}", e))?;

    // Load tokenizers
    let (source_tokenizer, target_tokenizer) = load_tokenizers(&config.model_dir)?;

    // Validate tokenizers are compatible with model special-token ids. When tokenizer generation
    // falls back to an incorrect WordLevel config, translation quality collapses.
    validate_tokenizer(&source_tokenizer, &model_config)?;
    validate_tokenizer(&target_tokenizer, &model_config)?;

    plugin_info!(logger, "Helsinki model loaded successfully");

    let translator = Arc::new(Mutex::new(CachedTranslator {
        model,
        source_tokenizer,
        target_tokenizer,
        config: model_config,
        device,
    }));

    // Store in cache
    {
        let mut cache = TRANSLATOR_CACHE
            .lock()
            .map_err(|e| format!("Cache lock failed: {}", e))?;

        cache.insert(
            cache_key,
            CachedTranslatorEntry {
                translator: translator.clone(),
            },
        );
    }

    Ok(translator)
}

fn validate_tokenizer(tokenizer: &Tokenizer, cfg: &Config) -> Result<(), String> {
    let text = "tokenizer self-check";
    let enc = tokenizer
        .encode(text, true)
        .map_err(|e| format!("Tokenizer encode failed: {}", e))?;

    let ids = enc.get_ids();
    if ids.is_empty() {
        return Err("Tokenizer produced empty encoding".to_string());
    }

    // Ensure the tokenizer understands the model's special-token ids by round-tripping them.
    let specials = [cfg.eos_token_id, cfg.pad_token_id, cfg.decoder_start_token_id];
    let decoded = tokenizer
        .decode(&specials, false)
        .map_err(|e| format!("Tokenizer decode failed: {}", e))?;

    if decoded.trim().is_empty() {
        return Err(format!(
            "Tokenizer appears incompatible with model ids (decoded specials empty). \
             Please regenerate tokenizer.json via: just download-helsinki-models"
        ));
    }

    Ok(())
}

fn validate_tokenizer_json(path: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read tokenizer.json: {}", e))?;
    let json: JsonValue =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse tokenizer.json: {}", e))?;

    let model_type = json
        .get("model")
        .and_then(|m| m.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    // MarianTokenizerFast should produce a SentencePiece/Unigram-based tokenizer.
    // A WordLevel tokenizer here is a known-bad fallback that yields garbage translations.
    if model_type.eq_ignore_ascii_case("wordlevel") {
        return Err(
            "tokenizer.json is WordLevel (fallback) and is not compatible with Marian OPUS-MT. \
             Regenerate models with: just download-helsinki-models"
                .to_string(),
        );
    }

    Ok(())
}
