// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Explicit discovery metadata for sample pipelines.
//!
//! The Convert and Stream views group near-duplicate samples (e.g. the
//! colorbars codec/hardware family) into a single card with a variant selector,
//! and expose faceted/fuzzy search over capability tags and categories. All of
//! this is driven by metadata authored directly in each sample's YAML
//! (`group`/`variant`/`canonical`/`category`/`tags`/`keywords`) — there is no
//! runtime derivation from filenames or node-kind substrings. Bundled samples
//! are required to carry a consistent set of fields, enforced by
//! `apps/skit/tests/sample_discovery_metadata_test.rs` so a missing or
//! inconsistent field breaks CI rather than silently degrading the UI.

/// Discovery metadata parsed from a sample's YAML.
#[derive(Debug, Default, Clone)]
pub struct Discovery {
    pub group: Option<String>,
    pub variant: Option<String>,
    pub canonical: bool,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
}

fn push_term(terms: &mut Vec<String>, value: &str) {
    let term = value.trim().to_lowercase();
    if !term.is_empty() && !terms.contains(&term) {
        terms.push(term);
    }
}

/// Builds the backend search document the UI matches queries against.
///
/// Combines the human-facing fields with authored keywords and the pipeline's
/// node kinds (with `::`/`_` separators flattened to spaces so individual
/// segments like `whisper` or `vaapi` are matchable). Lowercased and
/// de-duplicated.
pub fn build_search_terms(
    name: &str,
    description: &str,
    discovery: &Discovery,
    node_kinds: &[String],
) -> Vec<String> {
    let mut terms = Vec::new();

    push_term(&mut terms, name);
    push_term(&mut terms, description);
    if let Some(category) = discovery.category.as_deref() {
        push_term(&mut terms, category);
    }
    if let Some(group) = discovery.group.as_deref() {
        push_term(&mut terms, group);
    }
    if let Some(variant) = discovery.variant.as_deref() {
        push_term(&mut terms, variant);
    }
    for tag in &discovery.tags {
        push_term(&mut terms, tag);
    }
    for keyword in &discovery.keywords {
        push_term(&mut terms, keyword);
    }
    for kind in node_kinds {
        push_term(&mut terms, &kind.replace("::", " ").replace('_', " "));
    }

    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn search_terms_include_authored_metadata() {
        let discovery = Discovery {
            group: Some("video-colorbars".to_string()),
            variant: Some("VA-API H.264".to_string()),
            canonical: false,
            category: Some("Video Encoding".to_string()),
            tags: vec!["video-encoding".to_string(), "hardware:vaapi".to_string()],
            keywords: vec!["test pattern".to_string(), "smpte".to_string()],
        };
        let terms = build_search_terms(
            "Video Color Bars (VA-API H.264)",
            "Encodes SMPTE color bars",
            &discovery,
            &kinds(&["video::vaapi::h264_encoder"]),
        );

        assert!(terms.contains(&"video encoding".to_string()));
        assert!(terms.contains(&"hardware:vaapi".to_string()));
        assert!(terms.contains(&"test pattern".to_string()));
        assert!(terms.contains(&"video vaapi h264 encoder".to_string()));
    }

    #[test]
    fn search_terms_are_lowercased_and_deduped() {
        let discovery = Discovery {
            category: Some("Streaming".to_string()),
            tags: vec!["streaming".to_string()],
            ..Discovery::default()
        };
        let terms = build_search_terms("Streaming", "STREAMING", &discovery, &[]);

        assert_eq!(terms.iter().filter(|t| *t == "streaming").count(), 1);
        assert!(terms.iter().all(|t| t == &t.to_lowercase()));
    }

    #[test]
    fn search_terms_skip_absent_optional_fields() {
        let discovery = Discovery::default();
        let terms = build_search_terms("Solo Sample", "", &discovery, &[]);
        assert_eq!(terms, vec!["solo sample".to_string()]);
    }
}
