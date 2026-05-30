// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Guards every shipped dynamic sample against referencing a node kind that
//! the server can't provide. The YAML compiler is registry-agnostic (it only
//! checks graph structure), so a renamed or mistyped node kind compiles fine
//! yet fails at runtime when the engine asks the registry to build the node —
//! a failure mode that produces no clear error (the session is created, the
//! node just never starts). This test closes that gap at build time.
//!
//! Every kind referenced by a sample must be either registered in the default
//! node registry, or a known *external* kind — a marketplace plugin
//! (`plugin::…`) or a codec behind a non-default cargo feature / dedicated
//! hardware. Anything else (e.g. a typo) fails the test.

// Test-fixture checks should fail fast with contextual assertion messages.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use streamkit_api::yaml::{parse_yaml, UserPipeline};
use streamkit_core::{GlobalNodeConstraints, NodeRegistry};

/// Node kinds that are intentionally absent from the default build: codecs
/// gated behind non-default cargo features (`svt_av1`, `dav1d`) or dedicated
/// GPU hardware (`nvcodec`, `vaapi`, `vulkan_video`). Samples may reference
/// these; real execution happens in feature-enabled builds or on hardware.
const FEATURE_GATED_KINDS: &[&str] = &[
    "video::svt_av1::encoder",
    "video::dav1d::decoder",
    "video::nv::av1_encoder",
    "video::vaapi::av1_encoder",
    "video::vaapi::h264_encoder",
    "video::vulkan_video::h264_encoder",
];

fn dynamic_samples_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("streamkit-server should live under workspace_root/apps/skit")
        .join("samples/pipelines/dynamic")
}

fn node_kinds(pipeline: &UserPipeline) -> Vec<String> {
    match pipeline {
        UserPipeline::Steps { steps, .. } => steps.iter().map(|s| s.kind.clone()).collect(),
        UserPipeline::Dag { nodes, .. } => nodes.values().map(|n| n.kind.clone()).collect(),
    }
}

fn is_known_external(kind: &str) -> bool {
    kind.starts_with("plugin::") || FEATURE_GATED_KINDS.contains(&kind)
}

#[test]
fn every_dynamic_sample_node_kind_is_available_or_known() {
    let mut registry = NodeRegistry::new();
    streamkit_nodes::register_nodes(&mut registry, &GlobalNodeConstraints::new());

    let dir = dynamic_samples_dir();
    let mut checked = 0usize;
    let mut failures = Vec::new();
    let mut unverified_external: BTreeSet<String> = BTreeSet::new();

    for entry in std::fs::read_dir(&dir).expect("dynamic samples directory should exist") {
        let path = entry.expect("readable dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "yml" && ext != "yaml" {
            continue;
        }
        let name = path.file_name().expect("file name").to_string_lossy().into_owned();
        let yaml = std::fs::read_to_string(&path).expect("sample readable");
        let pipeline = match parse_yaml(&yaml) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{name}: parse failed: {e}"));
                continue;
            },
        };
        checked += 1;

        for kind in node_kinds(&pipeline) {
            if registry.contains(&kind) {
                continue;
            }
            if is_known_external(&kind) {
                unverified_external.insert(kind);
                continue;
            }
            failures.push(format!(
                "{name}: node kind '{kind}' is not registered in the default build and is not a \
                 known plugin/feature-gated kind — typo, or add it to FEATURE_GATED_KINDS"
            ));
        }
    }

    assert!(checked > 0, "expected to find dynamic sample files in {}", dir.display());
    assert!(failures.is_empty(), "node-availability check failed:\n{}", failures.join("\n"));

    // Every shipped sample either uses default nodes or a recognized external
    // kind; `unverified_external` records the latter for debugging context.
    assert!(
        unverified_external.iter().all(|k| is_known_external(k)),
        "unexpected external kinds: {unverified_external:?}"
    );
}
