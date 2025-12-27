#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

MODEL_NAME="vits-piper-es_MX-claude-high"
MODEL_DIR="models/${MODEL_NAME}"
BASE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models"

echo "Downloading Mexican Spanish Piper TTS model: ${MODEL_NAME}"
echo "Model directory: ${MODEL_DIR}"
echo

mkdir -p models

# Download pre-converted model from sherpa-onnx releases
echo "Downloading ${MODEL_NAME}.tar.bz2..."
cd models
if [ -f "${MODEL_NAME}.tar.bz2" ]; then
    echo "Archive already exists, skipping download."
else
    wget "${BASE_URL}/${MODEL_NAME}.tar.bz2"
fi

if [ -d "${MODEL_NAME}" ]; then
    echo "Models already extracted, skipping."
else
    echo "Extracting models..."
    tar xf "${MODEL_NAME}.tar.bz2"
fi

cd ..

echo
echo "✅ Spanish model downloaded successfully!"
echo "   Location: ${MODEL_DIR}"
echo "   Files:"
ls -lh "${MODEL_DIR}"
echo

# Create symlinks if needed
cd "${MODEL_DIR}"
if [ ! -f "model.onnx" ] && [ -f "es_MX-claude-high.onnx" ]; then
    echo "Creating symlinks for piper model naming..."
    ln -sf es_MX-claude-high.onnx model.onnx
    echo "  model.onnx -> es_MX-claude-high.onnx"
fi
cd - > /dev/null

echo
echo "Model info:"
echo "  - Language: Spanish (Mexico)"
echo "  - Voice: claude"
echo "  - Quality: high"
echo "  - Optimized for sherpa-onnx"
echo "Ready to use!"
