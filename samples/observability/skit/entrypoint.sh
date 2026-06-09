#!/bin/sh
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# Older -demo images (<= v0.5.0) ship native plugins as bare `.so` files under
# plugins/native/, but the loader expects directory bundles
# (plugins/native/<id>/ with a plugin.yml + the .so). Without this, `skit serve`
# logs "no plugins found" and TTS/STT pipelines fail with "node kind not found".
# We pass through any directory bundles the image already ships (newer images,
# see https://github.com/streamer45/streamkit/issues/553) and assemble the rest
# from the repo manifests (mounted at /repo-manifests) plus the bare .so files
# baked into the image, then start the server. Remove this shim once the pinned
# image ships plugin bundles.
set -e

SRC=/opt/streamkit/plugins/native
DST=/opt/streamkit/np/native
mkdir -p "$DST"

for dir in "$SRC"/*/; do
  [ -d "$dir" ] || continue
  id=$(basename "$dir")
  [ -d "$DST/$id" ] && continue
  if [ -f "$dir/plugin.yml" ] && ls "$dir"/*.so > /dev/null 2>&1; then
    cp -r "$SRC/$id" "$DST/$id"
    echo "copied plugin bundle: $id"
  fi
done

for manifest in /repo-manifests/*/plugin.yml; do
  [ -f "$manifest" ] || continue
  id=$(basename "$(dirname "$manifest")")
  [ -d "$DST/$id" ] && continue
  so=$(awk '/^entrypoint:/{print $2}' "$manifest" | tr -d '\r')
  if [ -n "$so" ] && [ -f "$SRC/$so" ]; then
    mkdir -p "$DST/$id"
    cp "$manifest" "$DST/$id/plugin.yml"
    cp "$SRC/$so" "$DST/$id/$so"
    echo "assembled plugin bundle: $id ($so)"
  fi
done

exec skit serve
