#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

plugins=(
  whisper
  kokoro
  piper
  matcha
  pocket-tts
  sensevoice
  nllb
  vad
  helsinki
)

for plugin in "${plugins[@]}"; do
  echo "Building native plugin: ${plugin}"
  (
    cd "plugins/native/${plugin}"
    cargo build --release
  )
done
