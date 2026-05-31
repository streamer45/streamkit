// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Enforces the explicit discovery-metadata contract for every bundled sample.
//!
//! The Convert/Stream pickers render group/variant/category/tags straight from
//! the sample YAML — there is no runtime derivation. This test is the single
//! source of truth that the authored metadata is present and internally
//! consistent, so a missing field or a malformed group fails CI loudly instead
//! of silently degrading the UI (e.g. an uncategorised card, or a group that
//! titles itself after an arbitrary member).

// Test-fixture checks should fail fast with contextual assertion messages.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use streamkit_api::yaml::{parse_yaml, UserPipeline};

/// A sample's authored discovery metadata, flattened across the `Steps`/`Dag`
/// pipeline arms.
struct SampleMeta {
    file: String,
    group: Option<String>,
    variant: Option<String>,
    canonical: bool,
    category: Option<String>,
    tags: Vec<String>,
}

fn samples_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("streamkit-server should live under workspace_root/apps/skit")
        .join("samples/pipelines")
}

/// Sample subdirectories surfaced in the Convert/Stream pickers. The `test/`
/// fixtures are validation-only and never listed, so they are excluded.
const DISCOVERABLE_SUBDIRS: &[&str] = &["dynamic", "oneshot"];

fn read_samples() -> Vec<SampleMeta> {
    let root = samples_root();
    let mut samples = Vec::new();

    for subdir in DISCOVERABLE_SUBDIRS {
        let dir = root.join(subdir);
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading {} failed: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "yml" && ext != "yaml" {
                continue;
            }
            let file =
                format!("{subdir}/{}", path.file_name().expect("file name").to_string_lossy());
            let yaml = std::fs::read_to_string(&path).expect("sample readable");
            let pipeline =
                parse_yaml(&yaml).unwrap_or_else(|e| panic!("{file}: parse failed: {e}"));

            let (UserPipeline::Steps { meta, .. } | UserPipeline::Dag { meta, .. }) = pipeline;

            samples.push(SampleMeta {
                file,
                group: meta.group,
                variant: meta.variant,
                canonical: meta.canonical,
                category: meta.category,
                tags: meta.tags,
            });
        }
    }

    assert!(!samples.is_empty(), "expected to find bundled sample files");
    samples
}

#[test]
fn every_bundled_sample_has_required_discovery_metadata() {
    let mut failures = Vec::new();

    for sample in read_samples() {
        match &sample.category {
            Some(c) if !c.trim().is_empty() => {},
            _ => failures.push(format!("{}: missing required `category`", sample.file)),
        }
        if sample.tags.iter().all(|t| t.trim().is_empty()) {
            failures.push(format!("{}: missing required `tags`", sample.file));
        } else if sample.tags.iter().any(|t| t.trim().is_empty()) {
            failures.push(format!("{}: has a blank `tags` entry", sample.file));
        }
    }

    assert!(failures.is_empty(), "discovery metadata check failed:\n{}", failures.join("\n"));
}

#[test]
fn grouped_samples_are_internally_consistent() {
    let samples = read_samples();
    let mut failures = Vec::new();
    let mut groups: BTreeMap<String, Vec<&SampleMeta>> = BTreeMap::new();

    for sample in &samples {
        match sample.group.as_deref().map(str::trim) {
            Some(group) if !group.is_empty() => {
                groups.entry(group.to_string()).or_default().push(sample);
            },
            _ => {
                // Ungrouped samples are standalone cards; the canonical flag and
                // a variant label are meaningless without a group to represent.
                if sample.canonical {
                    failures.push(format!(
                        "{}: sets `canonical: true` but has no `group`",
                        sample.file
                    ));
                }
                if sample.variant.as_deref().is_some_and(|v| !v.trim().is_empty()) {
                    failures.push(format!("{}: sets `variant` but has no `group`", sample.file));
                }
            },
        }
    }

    for (group, members) in &groups {
        if members.len() < 2 {
            let member = members[0];
            failures.push(format!(
                "{}: `group: {group}` has only one member; drop the group or add siblings",
                member.file
            ));
            continue;
        }

        let canonical_count = members.iter().filter(|m| m.canonical).count();
        if canonical_count != 1 {
            failures.push(format!(
                "group `{group}` must have exactly one `canonical: true` member, found \
                 {canonical_count} ({})",
                members.iter().map(|m| m.file.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }

        let mut seen_variants = BTreeSet::new();
        for member in members {
            match member.variant.as_deref().map(str::trim) {
                Some(variant) if !variant.is_empty() => {
                    if !seen_variants.insert(variant.to_string()) {
                        failures.push(format!(
                            "group `{group}` has duplicate `variant: {variant}`; variant labels \
                             must be unique within a group"
                        ));
                    }
                },
                _ => failures.push(format!(
                    "{}: member of multi-sample group `{group}` must set a `variant` label",
                    member.file
                )),
            }
        }
    }

    assert!(failures.is_empty(), "group consistency check failed:\n{}", failures.join("\n"));
}
