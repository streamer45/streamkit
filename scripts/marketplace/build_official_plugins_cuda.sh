#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

# Builds the CUDA-capable official plugins for the marketplace `cuda` variant.
#
# Plugins are selected from marketplace/official-plugins.json by their
# `accelerators` list containing "cuda". Plugins that declare a `cuda` Cargo
# feature (e.g. whisper, helsinki) are built with `--features cuda`; sherpa-onnx
# plugins are execution-provider agnostic, so their CPU `.so` is rebuilt as-is
# and later packaged against the GPU sherpa runtime by build_registry.py.

set -euo pipefail

python3 scripts/marketplace/generate_official_plugins.py

cuda_plugins=$(python3 - <<'PY'
import json
import pathlib

metadata = json.loads(pathlib.Path("marketplace/official-plugins.json").read_text())
for plugin in metadata.get("plugins", []):
    if "cuda" in plugin.get("accelerators", []):
        print(f"{plugin['id']}\t{plugin['artifact']}")
PY
)

if [ -z "${cuda_plugins}" ]; then
  echo "No CUDA-capable plugins declared in marketplace/official-plugins.json" >&2
  exit 0
fi

target_dir="${CARGO_TARGET_DIR:-$(pwd)/target/plugins}"

while IFS=$'\t' read -r plugin artifact; do
  if [ -z "${plugin}" ]; then
    continue
  fi
  plugin_dir="plugins/native/${plugin}"
  if [ ! -d "${plugin_dir}" ]; then
    echo "Missing plugin directory: ${plugin_dir}" >&2
    exit 1
  fi

  features=()
  has_cuda_feature=0
  if grep -qE '^[[:space:]]*cuda[[:space:]]*=' "${plugin_dir}/Cargo.toml"; then
    features=(--features cuda)
    has_cuda_feature=1
  fi

  echo "Building CUDA plugin: ${plugin} ${features[*]:-}"
  (
    cd "${plugin_dir}"
    CARGO_TARGET_DIR="${target_dir}" cargo build --release ${features[@]+"${features[@]}"}
  )

  # Guard against silently shipping an unaccelerated CPU build as a `cuda`
  # variant. A cuda-declared plugin must either compile its own `cuda` feature
  # or link the (execution-provider-agnostic) sherpa-onnx runtime that
  # build_registry.py later packages against the GPU sherpa libs. If neither
  # holds, the produced .so is a plain CPU build masquerading as cuda.
  artifact_path="${target_dir}/release/$(basename "${artifact}")"
  if [ "${has_cuda_feature}" -eq 0 ]; then
    if [ ! -f "${artifact_path}" ]; then
      echo "ERROR: expected artifact not found after build: ${artifact_path}" >&2
      exit 1
    fi
    if ! readelf -d "${artifact_path}" 2>/dev/null | grep -q 'libsherpa-onnx-c-api.so'; then
      echo "ERROR: plugin '${plugin}' declares the 'cuda' accelerator but has" \
           "neither a 'cuda' Cargo feature nor sherpa-onnx linkage; refusing to" \
           "package a plain CPU build as a cuda variant." >&2
      exit 1
    fi
  fi
done <<< "${cuda_plugins}"
