#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

# Navigate to repo root (two levels up from examples/matcha-plugin-native)
cd "$(dirname "$0")/../.."

MODEL_NAME="matcha-icefall-en_US-ljspeech"
MODEL_DIR="models/${MODEL_NAME}"
BASE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models"

echo "Downloading Matcha TTS model: ${MODEL_NAME}"
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

# Download Vocos vocoder (required, ~10 MB)
echo
echo "Downloading Vocos vocoder model..."
VOCODER_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/vocoder-models/vocos-22khz-univ.onnx"
VOCODER_PATH="${MODEL_DIR}/vocos-22khz-univ.onnx"

if [ -f "${VOCODER_PATH}" ]; then
    echo "Vocoder already exists, skipping download."
else
    wget -O "${VOCODER_PATH}" "${VOCODER_URL}"
    echo "✓ Vocoder downloaded to ${VOCODER_PATH}"
fi

echo
echo "✅ Model downloaded successfully!"
echo "   Location: ${MODEL_DIR}"
echo "   Files:"
ls -lh "${MODEL_DIR}"
echo

echo "Model info:"
echo "  - Sample rate: 22050 Hz"
echo "  - 1 speaker (LJSpeech dataset - female voice)"
echo "  - High-quality English speech"
echo "  - Optimized for sherpa-onnx"
echo "Ready to use!"
