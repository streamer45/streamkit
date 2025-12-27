#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

# Navigate to repo root (two levels up from examples/piper-plugin-native)
cd "$(dirname "$0")/../.."

MODEL_NAME="vits-piper-en_US-libritts_r-medium"
MODEL_DIR="models/${MODEL_NAME}"
BASE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models"

echo "Downloading Piper TTS model: ${MODEL_NAME}"
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
echo "✅ Model downloaded successfully!"
echo "   Location: ${MODEL_DIR}"
echo "   Files:"
ls -lh "${MODEL_DIR}"
echo

# Create symlinks if needed (piper models have different naming)
cd "${MODEL_DIR}"
if [ ! -f "model.onnx" ] && [ -f "en_US-libritts_r-medium.onnx" ]; then
    echo "Creating symlinks for piper model naming..."
    ln -sf en_US-libritts_r-medium.onnx model.onnx
    echo "  model.onnx -> en_US-libritts_r-medium.onnx"
fi
cd - > /dev/null

echo
echo "Model info:"
echo "  - Sample rate: 22050 Hz"
echo "  - 904 speakers (LibriTTS-R dataset)"
echo "  - High-quality natural speech"
echo "  - Optimized for sherpa-onnx"
echo "Ready to use!"
