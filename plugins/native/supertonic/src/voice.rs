// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use streamkit_plugin_sdk_native::prelude::*;
use supertonic_vendor::Style;

/// Wrapper to mark `Style` as thread-safe (it is read-only after creation).
pub struct StyleWrapper(pub Style);

// SAFETY: Style is read-only after construction and only contains ndarray data.
unsafe impl Send for StyleWrapper {}
unsafe impl Sync for StyleWrapper {}

/// Global cache of loaded voice styles, keyed by resolved absolute path.
static VOICE_CACHE: std::sync::LazyLock<Mutex<HashMap<String, Arc<StyleWrapper>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Resolve a voice style name or path to a loaded `Style`.
///
/// Resolution order:
/// 1. If `voice_style` ends with `.json`, treat as direct path
/// 2. If `voice_styles_dir` is set, look for `{dir}/{voice_style}.json`
/// 3. Fallback to `{model_dir}/voice_styles/{voice_style}.json`
pub fn resolve_voice_style(
    voice_style: &str,
    voice_styles_dir: Option<&str>,
    model_dir: &str,
    logger: &Logger,
) -> Result<Arc<StyleWrapper>, String> {
    let resolved_path = resolve_path(voice_style, voice_styles_dir, model_dir)?;

    let resolved_str = resolved_path.to_string_lossy().to_string();

    {
        let cache = VOICE_CACHE.lock().map_err(|e| format!("Failed to lock voice cache: {e}"))?;

        if let Some(cached) = cache.get(&resolved_str) {
            plugin_info!(
                logger,
                path = %resolved_str,
                "CACHE HIT: Reusing cached voice style"
            );
            return Ok(cached.clone());
        }
    }

    plugin_info!(
        logger,
        path = %resolved_str,
        "CACHE MISS: Loading voice style"
    );

    let style = supertonic_vendor::load_voice_style(std::slice::from_ref(&resolved_str))
        .map_err(|e| format!("Failed to load voice style '{resolved_str}': {e}"))?;

    let wrapper = Arc::new(StyleWrapper(style));
    VOICE_CACHE
        .lock()
        .map_err(|e| format!("Failed to lock voice cache: {e}"))?
        .insert(resolved_str, wrapper.clone());

    Ok(wrapper)
}

fn resolve_path(
    voice_style: &str,
    voice_styles_dir: Option<&str>,
    model_dir: &str,
) -> Result<PathBuf, String> {
    // 1. Direct .json path
    if Path::new(voice_style).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
        let p = Path::new(voice_style);
        if p.exists() {
            return p
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize voice style path: {e}"));
        }
        return Err(format!("Voice style file not found: {voice_style}"));
    }

    // 2. Named style in voice_styles_dir
    if let Some(dir) = voice_styles_dir {
        let p = Path::new(dir).join(format!("{voice_style}.json"));
        if p.exists() {
            return p
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize voice style path: {e}"));
        }
    }

    // 3. Fallback to model_dir/voice_styles/
    let p = Path::new(model_dir).join("voice_styles").join(format!("{voice_style}.json"));
    if p.exists() {
        return p
            .canonicalize()
            .map_err(|e| format!("Failed to canonicalize voice style path: {e}"));
    }

    Err(format!(
        "Voice style '{voice_style}' not found. Searched: voice_styles_dir={voice_styles_dir:?}, model_dir={model_dir}/voice_styles/"
    ))
}
