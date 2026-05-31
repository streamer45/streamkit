#!/bin/sh
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# The -demo images currently ship native plugins as bare `.so` files under
# plugins/native/, but the loader expects directory bundles
# (plugins/native/<id>/ with a plugin.yml + the .so). Without this, `skit serve`
# logs "no plugins found" and TTS/STT pipelines fail with "node kind not found".
# We assemble the expected layout from the repo manifests (mounted at
# /repo-manifests) plus the .so files baked into the image, then start the
# server. Tracked upstream; remove once the demo image ships bundles directly.
set -e

SRC=/opt/streamkit/plugins/native
DST=/opt/streamkit/np/native
mkdir -p "$DST"

for manifest in /repo-manifests/*/plugin.yml; do
  [ -f "$manifest" ] || continue
  id=$(basename "$(dirname "$manifest")")
  so=$(awk '/^entrypoint:/{print $2}' "$manifest")
  if [ -n "$so" ] && [ -f "$SRC/$so" ]; then
    mkdir -p "$DST/$id"
    cp "$manifest" "$DST/$id/plugin.yml"
    cp "$SRC/$so" "$DST/$id/$so"
    echo "assembled plugin bundle: $id ($so)"
  fi
done

exec skit serve
