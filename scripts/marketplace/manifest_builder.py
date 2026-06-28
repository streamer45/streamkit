#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""Shared manifest/registry helpers.

The registry is append-only: a committed manifest must round-trip byte-identically
when rebuilt from plugin.yml. `build_registry.py` (the builder) and
`check_registry_versions.py` (the pre-merge guard) therefore have to agree on the
exact manifest shape, and `verify_bundles.py` has to agree with the builder on
which GPU libraries a cuda bundle carries. Keeping those definitions here makes a
single source of truth instead of byte-fragile copies.
"""

# Core sherpa runtime libraries vendored into every sherpa-backed bundle.
SHERPA_CORE_LIBS = ["libsherpa-onnx-c-api.so", "libonnxruntime.so"]

# Additional ONNX Runtime execution-provider libraries required when the
# vendored libonnxruntime.so is the CUDA-enabled build. The same plugin `.so`
# loads these at runtime to dispatch to the GPU.
SHERPA_CUDA_LIBS = [
    "libonnxruntime_providers_cuda.so",
    "libonnxruntime_providers_shared.so",
]


def strip_none(payload: dict) -> dict:
    return {key: value for key, value in payload.items() if value is not None}


def build_manifest(
    plugin: dict,
    version: str,
    bundle_block: dict | None,
    variants: list[dict] | None = None,
) -> dict:
    """Build the registry manifest dict from plugin metadata and bundle info.

    `variants` carries accelerator-specific bundles (e.g. a CUDA build) and is
    inserted right after `bundle` for readable diffs; it is omitted entirely when
    empty so CPU-only manifests stay byte-identical to those produced before
    variant support existed.
    """
    manifest = {
        "schema_version": 1,
        "id": plugin["id"],
        "name": plugin.get("name"),
        "version": version,
        "node_kind": plugin["node_kind"],
        "kind": plugin["kind"],
        "description": plugin.get("description"),
        "license": plugin.get("license"),
        "license_url": plugin.get("license_url"),
        "homepage": plugin.get("homepage"),
        "repository": plugin.get("repo"),
        "entrypoint": plugin["entrypoint"],
        "bundle": bundle_block,
        "compatibility": plugin.get("compatibility"),
        "models": plugin.get("models", []),
        "assets": plugin.get("assets") or None,
    }
    manifest = strip_none(manifest)
    if variants:
        ordered: dict = {}
        for key, value in manifest.items():
            ordered[key] = value
            if key == "bundle":
                ordered["variants"] = variants
        manifest = ordered
    return manifest
