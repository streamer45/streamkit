// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Translation inference logic for Helsinki-NLP OPUS-MT.

use std::sync::{Arc, Mutex};

use candle_core::{IndexOp, Tensor, D};

use crate::config::HelsinkiConfig;
use crate::model::CachedTranslator;

/// Translate text using the cached translator.
pub fn translate(
    translator: &Arc<Mutex<CachedTranslator>>,
    text: &str,
    config: &HelsinkiConfig,
) -> Result<String, String> {
    let mut translator = translator
        .lock()
        .map_err(|e| format!("Failed to lock translator: {}", e))?;

    // Reset KV cache for new sequence
    translator.model.reset_kv_cache();

    // Tokenize input text
    let encoding = translator
        .source_tokenizer
        .encode(text, false)
        .map_err(|e| format!("Tokenization failed: {}", e))?;

    let mut input_ids: Vec<u32> = encoding.get_ids().to_vec();
    if input_ids.is_empty() {
        return Ok(String::new());
    }
    // Marian models expect an EOS terminator on the encoder side. Do this explicitly so we don't
    // depend on tokenizer post-processing configuration.
    if input_ids.last().copied() != Some(translator.config.eos_token_id) {
        input_ids.push(translator.config.eos_token_id);
    }

    // Convert to tensor
    let input_tensor = Tensor::new(&input_ids[..], &translator.device)
        .map_err(|e| format!("Failed to create input tensor: {}", e))?
        .unsqueeze(0)
        .map_err(|e| format!("Failed to unsqueeze input: {}", e))?;

    // Run encoder
    let encoder_output = translator
        .model
        .encoder()
        .forward(&input_tensor, 0)
        .map_err(|e| format!("Encoder forward failed: {}", e))?;

    // Autoregressive decoding
    let decoder_start_token_id = translator.config.decoder_start_token_id;
    let eos_token_id = translator.config.eos_token_id;
    let pad_token_id = translator.config.pad_token_id;

    let mut decoder_input = vec![decoder_start_token_id];

    for step in 0..config.max_length {
        // When `use_cache` is enabled (default for Marian), Candle's attention layers maintain a KV
        // cache. To avoid duplicating cached keys/values (and shape mismatches in the causal mask),
        // we must feed only the newest token at each step.
        let input_token = *decoder_input
            .last()
            .ok_or_else(|| "Internal error: decoder_input is empty".to_string())?;
        let decoder_tensor = Tensor::new(&[input_token], &translator.device)
            .map_err(|e| format!("Failed to create decoder tensor: {}", e))?
            .unsqueeze(0)
            .map_err(|e| format!("Failed to unsqueeze decoder input: {}", e))?;

        // Run decoder
        let logits = translator
            .model
            .decode(&decoder_tensor, &encoder_output, step)
            .map_err(|e| format!("Decoder forward failed: {}", e))?;

        // Get last token logits (shape: [batch, seq_len, vocab])
        let seq_len = logits.dim(1).map_err(|e| format!("Failed to get dim: {}", e))?;
        let last_logits = logits
            .i((.., seq_len - 1, ..))
            .map_err(|e| format!("Failed to slice logits: {}", e))?;

        // Greedy sampling: take argmax
        let next_token = last_logits
            .argmax(D::Minus1)
            .map_err(|e| format!("Argmax failed: {}", e))?
            .squeeze(0)
            .map_err(|e| format!("Squeeze failed: {}", e))?
            .to_scalar::<u32>()
            .map_err(|e| format!("to_scalar failed: {}", e))?;

        // Check for EOS
        if next_token == eos_token_id || next_token == pad_token_id {
            break;
        }

        decoder_input.push(next_token);
    }

    // Decode output tokens (skip decoder start token)
    let output_ids: Vec<u32> = decoder_input
        .into_iter()
        .skip(1) // Skip decoder start token
        .filter(|&id| id != eos_token_id && id != pad_token_id)
        .collect();

    let decoded = translator
        .target_tokenizer
        .decode(&output_ids, true)
        .map_err(|e| format!("Decoding failed: {}", e))?;

    Ok(decoded.trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::ptr;

    use streamkit_plugin_sdk_native::prelude::{CLogCallback, CLogLevel, Logger};

    use crate::config::HelsinkiConfig;
    use crate::model::get_or_load_translator;
    use crate::translation::translate;

    extern "C" fn test_log_callback(
        _level: CLogLevel,
        _target: *const std::os::raw::c_char,
        _message: *const std::os::raw::c_char,
        _user_data: *mut std::os::raw::c_void,
    ) {
    }

    #[test]
    #[ignore = "requires local model files in ./models (run `just download-helsinki-models`)"]
    fn translate_smoke_en_es() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model_dir = repo_root.join("models/opus-mt-en-es");
        assert!(
            model_dir.join("model.safetensors").exists(),
            "Missing model files at {}",
            model_dir.display()
        );

        let logger = Logger::new(test_log_callback as CLogCallback, ptr::null_mut(), "helsinki");

        let mut config = HelsinkiConfig::default();
        config.model_dir = model_dir.to_string_lossy().to_string();
        config.max_length = 64;
        config.validate().unwrap();

        let translator = get_or_load_translator(&config, &logger).unwrap();
        let output = translate(&translator, "Hello world!", &config).unwrap();
        println!("translated: {output}");
        assert!(!output.trim().is_empty());
    }
}
