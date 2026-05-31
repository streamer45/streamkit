// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Best-effort discovery metadata for sample pipelines.
//!
//! The Convert and Stream views group near-duplicate samples (e.g. the
//! h264/av1 colorbars family) into a single card with a variant selector, and
//! expose faceted/fuzzy search over capability tags and categories. Authors may
//! set `group`/`variant`/`category`/`tags` explicitly in a sample's YAML, but
//! most samples omit them — so this module derives sensible defaults from the
//! node kinds, the client section, and the filename. Explicit YAML values
//! always win; derived `tags` are unioned with any curated ones.

use streamkit_api::yaml::{InputType, OutputType};
use streamkit_core::types::{AudioCodec, VideoCodec};

/// Output codec a video encoder node produces, or `None` for non-encoder kinds.
///
/// Node kinds embed the codec's canonical name (`video::vp9::encoder`,
/// `video::vaapi::av1_encoder`, `video::vulkan_video::h264_encoder`, ...), so we
/// match against [`VideoCodec::as_c_name`] rather than re-spelling codec strings
/// — adding a codec variant is then a single edit on the enum.
fn video_codec_for_kind(kind: &str) -> Option<VideoCodec> {
    if !kind.contains("encoder") {
        return None;
    }
    [VideoCodec::Vp9, VideoCodec::H264, VideoCodec::Av1]
        .into_iter()
        .find(|codec| kind.contains(codec.as_c_name()))
}

/// Output codec an audio encoder node produces, or `None` for non-encoder kinds.
fn audio_codec_for_kind(kind: &str) -> Option<AudioCodec> {
    if !kind.contains("encoder") {
        return None;
    }
    [AudioCodec::Opus, AudioCodec::Aac].into_iter().find(|codec| kind.contains(codec.as_c_name()))
}

/// Discovery metadata for a sample. Used both for the explicit values parsed
/// from a sample's YAML (all optional) and for the resolved values after
/// merging those with the derived defaults.
#[derive(Debug, Default, Clone)]
pub struct Discovery {
    pub group: Option<String>,
    pub variant: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
}

/// Filename tokens that distinguish variants of the same scenario, paired with
/// a human label. Compound tokens are matched before single ones so that e.g.
/// `vulkan_video` does not collapse into a stray `video` group token.
const COMPOUND_TOKENS: &[(&str, &str)] =
    &[("vulkan_video", "Vulkan Video"), ("svt_av1", "SVT-AV1")];

const SINGLE_TOKENS: &[(&str, &str)] = &[
    ("h264", "H.264"),
    ("h265", "HEVC"),
    ("hevc", "HEVC"),
    ("av1", "AV1"),
    ("vp9", "VP9"),
    ("aac", "AAC"),
    ("opus", "Opus"),
    ("vaapi", "VA-API"),
    ("nvidia", "NVIDIA"),
    ("nvenc", "NVIDIA"),
    ("nv", "NVIDIA"),
    ("vulkan", "Vulkan"),
    ("dav1d", "dav1d"),
    ("svt", "SVT"),
    ("openh264", "OpenH264"),
    ("helsinki", "Helsinki"),
    ("nllb", "NLLB"),
];

const LANGUAGE_TOKENS: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
];

fn label_for(table: &[(&'static str, &'static str)], token: &str) -> Option<&'static str> {
    table.iter().find(|(t, _)| *t == token).map(|(_, label)| *label)
}

/// Splits a filename base into its base-scenario group key and a variant label,
/// stripping codec/hardware/language tokens that distinguish near-duplicates.
///
/// Language tokens are only honoured when `is_translation` is set; the two-letter
/// codes (`de`, `it`, `pt`, ...) collide with common filename fragments, so
/// outside a translation pipeline they stay part of the group key.
fn group_and_variant_from_filename(
    filename_base: &str,
    is_translation: bool,
) -> (String, Option<String>) {
    let mut normalized = filename_base.to_lowercase().replace('-', "_");

    let mut variant_parts: Vec<String> = Vec::new();

    for (token, label) in COMPOUND_TOKENS {
        let needle = format!("_{token}_");
        let padded = format!("_{normalized}_");
        if padded.contains(&needle) {
            normalized = padded.replace(&needle, "_").trim_matches('_').to_string();
            variant_parts.push((*label).to_string());
        }
    }

    let mut group_tokens: Vec<String> = Vec::new();
    let mut languages: Vec<&'static str> = Vec::new();

    for token in normalized.split('_').filter(|t| !t.is_empty()) {
        if let Some(lang) = is_translation.then(|| label_for(LANGUAGE_TOKENS, token)).flatten() {
            languages.push(lang);
        } else if let Some(label) = label_for(SINGLE_TOKENS, token) {
            variant_parts.push(label.to_string());
        } else {
            group_tokens.push(token.to_string());
        }
    }

    if languages.len() >= 2 {
        variant_parts.push(format!("{} → {}", languages[0], languages[1]));
    } else if let Some(lang) = languages.first() {
        variant_parts.push((*lang).to_string());
    }

    let group_key = if group_tokens.is_empty() {
        filename_base.to_lowercase().replace('_', "-")
    } else {
        group_tokens.join("-")
    };

    let variant = if variant_parts.is_empty() { None } else { Some(variant_parts.join(" ")) };

    (group_key, variant)
}

fn tags_for_kind(k: &str) -> Vec<&'static str> {
    let mut tags: Vec<&'static str> = Vec::new();

    if k.contains("whisper") || k.contains("parakeet") || k.contains("sensevoice") {
        tags.push("speech-to-text");
    }
    if k.contains("kokoro")
        || k.contains("piper")
        || k.contains("matcha")
        || k.contains("supertonic")
        || k.contains("pocket")
    {
        tags.push("text-to-speech");
    }
    if k.contains("nllb") || k.contains("helsinki") {
        tags.push("translation");
    }
    if k.ends_with("::vad") {
        tags.push("voice-activity-detection");
    }
    if k.starts_with("video::") && k.contains("encoder") {
        tags.push("video-encoding");
    }
    if k.starts_with("video::") && k.contains("decoder") {
        tags.push("video-decoding");
    }
    if k == "video::compositor" {
        tags.push("compositing");
    }
    if k == "video::colorbars" {
        tags.push("colorbars");
    }
    if k.starts_with("transport::moq") {
        tags.push("moq");
    }
    if k == "transport::http::mse" {
        tags.push("mse");
    }
    if k.starts_with("transport::rtmp") {
        tags.push("rtmp");
    }
    if k.starts_with("containers::mp4") {
        tags.push("mp4");
    }
    if k.starts_with("containers::webm") {
        tags.push("webm");
    }
    if k == "audio::mixer" {
        tags.push("mixing");
    }

    if k.contains("vaapi") {
        tags.push("hardware:vaapi");
    }
    if k.starts_with("video::nv::") || k.contains("nvenc") {
        tags.push("hardware:nvidia");
    }
    if k.contains("vulkan") {
        tags.push("hardware:vulkan");
    }

    tags
}

fn capability_tags(
    node_kinds: &[String],
    client_input: Option<InputType>,
    client_output: Option<OutputType>,
) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();

    for kind in node_kinds {
        let kind = kind.to_lowercase();
        for tag in tags_for_kind(&kind) {
            tags.push(tag.to_string());
        }
        // Output codec, surfaced as the variant axis (pills) and a search term
        // but excluded from the capability facets (see collectSampleFacets).
        let codec = video_codec_for_kind(&kind)
            .map(VideoCodec::as_c_name)
            .or_else(|| audio_codec_for_kind(&kind).map(AudioCodec::as_c_name));
        if let Some(name) = codec {
            tags.push(format!("codec:{name}"));
        }
    }

    match client_output {
        Some(OutputType::Transcription) => tags.push("speech-to-text".to_string()),
        Some(OutputType::Audio) => tags.push("audio-output".to_string()),
        Some(OutputType::Video) => tags.push("video-output".to_string()),
        Some(OutputType::Json) | None => {},
    }
    match client_input {
        Some(InputType::FileUpload) => tags.push("file-input".to_string()),
        Some(InputType::Text) => tags.push("text-input".to_string()),
        Some(InputType::Trigger | InputType::None) | None => {},
    }

    tags.sort_unstable();
    tags.dedup();
    tags
}

/// Picks a single faceting bucket. Composition and encoding outrank the speech
/// stack so that a streaming pipeline that happens to transcribe still reads as
/// a video scenario; translation outranks STT/TTS for speech-translate samples.
fn category_from_tags(tags: &[String]) -> Option<String> {
    const PRIORITY: &[(&str, &str)] = &[
        ("compositing", "Video Compositing"),
        ("video-encoding", "Video Encoding"),
        ("translation", "Translation"),
        ("speech-to-text", "Speech to Text"),
        ("text-to-speech", "Text to Speech"),
        ("video-decoding", "Video Processing"),
        ("moq", "Streaming"),
        ("mse", "Streaming"),
        ("rtmp", "Streaming"),
        ("mixing", "Audio Processing"),
    ];

    PRIORITY
        .iter()
        .find(|(tag, _)| tags.iter().any(|t| t == tag))
        .map(|(_, category)| (*category).to_string())
}

/// Derives discovery metadata, with explicit YAML values taking precedence.
///
/// Grouping is only auto-derived for curated system samples; user pipelines
/// group solely via an explicit `group`, so two unrelated saves whose names
/// happen to share a group token are never silently collapsed into one card.
pub fn derive(
    filename_base: &str,
    node_kinds: &[String],
    client_input: Option<InputType>,
    client_output: Option<OutputType>,
    is_system: bool,
    explicit: Discovery,
) -> Discovery {
    let derived_tags = capability_tags(node_kinds, client_input, client_output);

    let mut tags = explicit.tags;
    for tag in derived_tags {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags.sort_unstable();
    tags.dedup();

    let is_translation = tags.iter().any(|t| t == "translation");
    let (derived_group, derived_variant) =
        group_and_variant_from_filename(filename_base, is_translation);

    let category = explicit.category.or_else(|| category_from_tags(&tags));

    Discovery {
        group: explicit.group.or(if is_system { Some(derived_group) } else { None }),
        variant: explicit.variant.or(derived_variant),
        category,
        tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn colorbars_family_shares_a_group_with_distinct_variants() {
        let (g_plain, v_plain) = group_and_variant_from_filename("video_moq_colorbars", false);
        let (g_h264, v_h264) = group_and_variant_from_filename("video_moq_h264_colorbars", false);
        let (g_vaapi, v_vaapi) =
            group_and_variant_from_filename("video_moq_vaapi_h264_colorbars", false);
        let (g_nv, v_nv) = group_and_variant_from_filename("video_moq_nv_av1_colorbars", false);
        let (g_vk, v_vk) =
            group_and_variant_from_filename("video_moq_vulkan_video_h264_colorbars", false);

        assert_eq!(g_plain, "video-moq-colorbars");
        assert_eq!(g_h264, "video-moq-colorbars");
        assert_eq!(g_vaapi, "video-moq-colorbars");
        assert_eq!(g_nv, "video-moq-colorbars");
        assert_eq!(g_vk, "video-moq-colorbars");

        assert_eq!(v_plain, None);
        assert_eq!(v_h264.as_deref(), Some("H.264"));
        assert_eq!(v_vaapi.as_deref(), Some("VA-API H.264"));
        assert_eq!(v_nv.as_deref(), Some("NVIDIA AV1"));
        assert_eq!(v_vk.as_deref(), Some("Vulkan Video H.264"));
    }

    #[test]
    fn svt_av1_compound_does_not_leak_into_group_key() {
        let (group, variant) =
            group_and_variant_from_filename("video_svt_av1_compositor_demo", false);
        assert_eq!(group, "video-compositor-demo");
        assert_eq!(variant.as_deref(), Some("SVT-AV1"));

        let (plain_group, plain_variant) =
            group_and_variant_from_filename("video_compositor_demo", false);
        assert_eq!(plain_group, "video-compositor-demo");
        assert_eq!(plain_variant, None);
    }

    #[test]
    fn language_pairs_become_directional_variants() {
        let (g_en_es, v_en_es) = group_and_variant_from_filename("speech-translate-en-es", true);
        let (g_es_en, v_es_en) = group_and_variant_from_filename("speech-translate-es-en", true);
        let (g_hel, v_hel) =
            group_and_variant_from_filename("speech-translate-helsinki-en-es", true);

        assert_eq!(g_en_es, "speech-translate");
        assert_eq!(g_es_en, "speech-translate");
        assert_eq!(g_hel, "speech-translate");
        assert_eq!(v_en_es.as_deref(), Some("English → Spanish"));
        assert_eq!(v_es_en.as_deref(), Some("Spanish → English"));
        assert_eq!(v_hel.as_deref(), Some("Helsinki English → Spanish"));
    }

    #[test]
    fn language_tokens_are_ignored_outside_a_translation_context() {
        let (group, variant) = group_and_variant_from_filename("video_de_interlace_demo", false);
        assert_eq!(group, "video-de-interlace-demo");
        assert_eq!(variant, None);
    }

    #[test]
    fn user_pipelines_are_not_auto_grouped() {
        let derive_for = |is_system| {
            derive(
                "encode_h264",
                &kinds(&["video::openh264::encoder"]),
                None,
                Some(OutputType::Video),
                is_system,
                Discovery::default(),
            )
            .group
        };
        assert_eq!(derive_for(true).as_deref(), Some("encode"));
        assert_eq!(derive_for(false), None);
    }

    #[test]
    fn unrelated_samples_do_not_collide() {
        let (g_audio, _) = group_and_variant_from_filename("mp4_mux_audio", false);
        let (g_video, _) = group_and_variant_from_filename("mp4_mux_video", false);
        let (g_av, _) = group_and_variant_from_filename("mp4_mux_aac_h264", false);
        assert_ne!(g_audio, g_video);
        assert_ne!(g_audio, g_av);
        assert_ne!(g_video, g_av);
    }

    #[test]
    fn derives_capability_tags_from_node_kinds() {
        let tags = capability_tags(
            &kinds(&[
                "streamkit::http_input",
                "audio::resampler",
                "plugin::native::whisper",
                "plugin::native::nllb",
                "plugin::native::piper",
            ]),
            None,
            None,
        );
        assert!(tags.contains(&"speech-to-text".to_string()));
        assert!(tags.contains(&"translation".to_string()));
        assert!(tags.contains(&"text-to-speech".to_string()));
    }

    #[test]
    fn flags_hardware_encoders_with_a_facet() {
        let vaapi = capability_tags(&kinds(&["video::vaapi::h264_encoder"]), None, None);
        assert!(vaapi.contains(&"video-encoding".to_string()));
        assert!(vaapi.contains(&"hardware:vaapi".to_string()));

        let nv = capability_tags(&kinds(&["video::nv::av1_encoder"]), None, None);
        assert!(nv.contains(&"hardware:nvidia".to_string()));

        let vulkan = capability_tags(&kinds(&["video::vulkan_video::h264_encoder"]), None, None);
        assert!(vulkan.contains(&"hardware:vulkan".to_string()));

        let software = capability_tags(&kinds(&["video::svt_av1::encoder"]), None, None);
        assert!(software.contains(&"video-encoding".to_string()));
        assert!(!software.iter().any(|t| t.starts_with("hardware:")));
    }

    #[test]
    fn aac_encoder_is_not_video_encoding() {
        let tags = capability_tags(&kinds(&["plugin::native::aac_encoder"]), None, None);
        assert!(!tags.contains(&"video-encoding".to_string()));
    }

    #[test]
    fn encoders_emit_the_output_codec_tag() {
        // The no-token base of a grouped family (e.g. the software colorbars or
        // the Opus mixer) carries no filename variant, so the UI labels its pill
        // from this codec tag instead of a hardcoded fallback.
        let vp9 = capability_tags(&kinds(&["video::vp9::encoder"]), None, None);
        assert!(vp9.contains(&"codec:vp9".to_string()));

        let opus = capability_tags(&kinds(&["audio::opus::encoder"]), None, None);
        assert!(opus.contains(&"codec:opus".to_string()));

        let h264 = capability_tags(&kinds(&["video::openh264::encoder"]), None, None);
        assert!(h264.contains(&"codec:h264".to_string()));

        let aac = capability_tags(&kinds(&["plugin::native::aac_encoder"]), None, None);
        assert!(aac.contains(&"codec:aac".to_string()));

        // Decoders are not the output codec.
        let decoder = capability_tags(&kinds(&["audio::opus::decoder"]), None, None);
        assert!(!decoder.iter().any(|t| t.starts_with("codec:")));
    }

    #[test]
    fn category_prefers_compositing_then_encoding() {
        assert_eq!(
            category_from_tags(&["compositing".into(), "video-encoding".into(), "moq".into()]),
            Some("Video Compositing".to_string())
        );
        assert_eq!(
            category_from_tags(&["video-encoding".into(), "moq".into()]),
            Some("Video Encoding".to_string())
        );
        assert_eq!(
            category_from_tags(&["translation".into(), "speech-to-text".into()]),
            Some("Translation".to_string())
        );
        assert_eq!(category_from_tags(&[]), None);
    }

    #[test]
    fn explicit_values_override_derivation_and_tags_union() {
        let discovery = derive(
            "video_moq_vaapi_h264_colorbars",
            &kinds(&["video::vaapi::h264_encoder", "transport::moq::publisher"]),
            None,
            Some(OutputType::Video),
            true,
            Discovery {
                group: Some("custom-group".to_string()),
                variant: Some("Custom".to_string()),
                category: Some("Demos".to_string()),
                tags: vec!["curated".to_string()],
            },
        );

        assert_eq!(discovery.group.as_deref(), Some("custom-group"));
        assert_eq!(discovery.variant.as_deref(), Some("Custom"));
        assert_eq!(discovery.category.as_deref(), Some("Demos"));
        assert!(discovery.tags.contains(&"curated".to_string()));
        assert!(discovery.tags.contains(&"video-encoding".to_string()));
        assert!(discovery.tags.contains(&"hardware:vaapi".to_string()));
    }

    #[test]
    fn derivation_fills_in_when_explicit_absent() {
        let discovery = derive(
            "video_moq_nv_av1_colorbars",
            &kinds(&["video::nv::av1_encoder", "transport::moq::publisher"]),
            None,
            Some(OutputType::Video),
            true,
            Discovery::default(),
        );
        assert_eq!(discovery.group.as_deref(), Some("video-moq-colorbars"));
        assert_eq!(discovery.variant.as_deref(), Some("NVIDIA AV1"));
        assert_eq!(discovery.category.as_deref(), Some("Video Encoding"));
        assert!(discovery.tags.contains(&"hardware:nvidia".to_string()));
    }
}
