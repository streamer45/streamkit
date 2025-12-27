<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Helsinki-NLP OPUS-MT Translation Plugin

A native plugin for real-time neural machine translation using Helsinki-NLP OPUS-MT models via Candle (pure Rust ML framework).

## Features

- **Apache 2.0 Licensed Models**: Suitable for commercial deployments
- **Bidirectional EN<->ES**: English to Spanish and Spanish to English translation
- **Pure Rust**: No C/C++ dependencies, powered by Candle ML framework
- **CPU & GPU Support**: Runs on CPU with optional CUDA acceleration
- **Model Caching**: Automatic model sharing across pipeline instances
- **Real-Time Ready**: Optimized for streaming translation pipelines

## License

**Helsinki OPUS-MT models are Apache 2.0 licensed** - suitable for both commercial and non-commercial use.

This is a key advantage over NLLB-200 (CC-BY-NC-4.0 non-commercial only).

## Quick Start

### 1. Install Dependencies

```bash
# Install Python tools for model conversion
pip3 install --user transformers sentencepiece safetensors torch tokenizers
```

### 2. Download and Convert OPUS-MT Models

The download script automatically converts models to Candle-compatible format (safetensors + tokenizer.json).

```bash
# Download and convert EN<->ES models (~600 MB total)
just download-helsinki-models
```

This downloads models from HuggingFace and converts:
- PyTorch weights → safetensors format (for Candle)
- SentencePiece + vocab.json → `source_tokenizer.json` + `target_tokenizer.json` (for Rust `tokenizers`)

If you see garbage/repetitive translations, regenerate the tokenizer JSON files (no re-download needed if model files already exist):

```bash
python3 plugins/native/helsinki/download-models.py
```

### 3. Build the Plugin

```bash
# From repository root
just build-plugin-native-helsinki

# Or manually
cd plugins/native/helsinki
cargo build --release

# Plugin binary:
# Linux: target/release/libhelsinki.so
# macOS: target/release/libhelsinki.dylib
# Windows: target/release/helsinki.dll
```

### 4. Upload to StreamKit Server

```bash
# Start server
just skit serve

# Upload plugin (in another terminal)
just upload-helsinki-plugin

# Or manually
curl -X POST \
  -F plugin=@plugins/native/helsinki/target/release/libhelsinki.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### 5. Use in Pipelines

The plugin will appear as `plugin::native::helsinki` in the node registry.

**Simple Translation Pipeline:**

```yaml
steps:
  - kind: streamkit::http_input

  - kind: plugin::native::helsinki
    params:
      model_dir: models/opus-mt-en-es
      source_language: en
      target_language: es
      device: cpu
      max_length: 512

  - kind: streamkit::http_output
```

**Complete Example Pipelines:**

- `samples/pipelines/dynamic/speech-translate-helsinki-en-es.yaml` - English speech → Spanish speech
- `samples/pipelines/dynamic/speech-translate-helsinki-es-en.yaml` - Spanish speech → English speech

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | `models/opus-mt-en-es` | Path to model directory |
| `source_language` | string | `en` | Source language: `en` or `es` |
| `target_language` | string | `es` | Target language: `en` or `es` |
| `device` | string | `cpu` | Device: `cpu`, `cuda`, or `auto` |
| `device_index` | integer | `0` | GPU device index (for `cuda`) |
| `max_length` | integer | `512` | Maximum output sequence length |

## Supported Language Pairs

| Model | Source | Target | Model Directory |
|-------|--------|--------|-----------------|
| opus-mt-en-es | English | Spanish | `models/opus-mt-en-es` |
| opus-mt-es-en | Spanish | English | `models/opus-mt-es-en` |

## GPU Support

### Build with CUDA

```bash
# Build with CUDA support
just build-plugin-native-helsinki-cuda

# Or manually
cd plugins/native/helsinki
cargo build --release --features cuda
```

### Use GPU in Pipeline

```yaml
params:
  model_dir: models/opus-mt-en-es
  source_language: en
  target_language: es
  device: cuda        # or "auto" to auto-detect
  device_index: 0     # GPU device ID
```

## Model Caching

The plugin automatically caches models across pipeline instances.

**Cache Key**: `(model_dir, device, device_index)`

**Cache Behavior:**
- Shared: model_dir, device settings
- Not Shared: source/target language, max_length (per-instance parameters)

## Comparison with NLLB

| Feature | Helsinki OPUS-MT | NLLB-200 |
|---------|-----------------|----------|
| **License** | Apache 2.0 | CC-BY-NC-4.0 (Non-commercial) |
| **Commercial Use** | Yes | No |
| **Languages** | 2 per model pair | 200+ |
| **Model Size** | ~300 MB per pair | ~600 MB - 6 GB |
| **Runtime** | Candle (pure Rust) | CTranslate2 (C++) |
| **Dependencies** | None (pure Rust) | libctranslate2 |
| **Quality (EN<->ES)** | Very Good | Good |

**When to use Helsinki OPUS-MT:**
- Commercial deployments
- EN<->ES translation only needed
- Prefer pure Rust dependencies
- Simpler deployment (no C++ libraries)

**When to use NLLB:**
- Non-commercial/research use
- Need 200+ languages
- Multilingual pipelines

## Architecture

### Model Loading Flow

1. Parse configuration from pipeline YAML
2. Validate language pair (en/es only)
3. Compute cache key: `(model_dir, device, device_index)`
4. Check global cache (`TRANSLATOR_CACHE`)
5. If cache miss: Load model weights + tokenizer → Cache it
6. If cache hit: Reuse existing `Arc<CachedTranslator>`

### Translation Flow

1. Receive `Packet::Text` or `Packet::Transcription` from upstream
2. Skip empty text
3. Lock translator for thread-safe access
4. Tokenize input with source tokenizer
5. Run encoder forward pass
6. Autoregressive decoding (greedy sampling)
7. Decode output tokens
8. Emit `Packet::Text` downstream

## References

- **Helsinki-NLP OPUS-MT**: https://huggingface.co/Helsinki-NLP
- **Candle ML Framework**: https://github.com/huggingface/candle
- **Marian-MT Paper**: https://aclanthology.org/P18-4020/
- **OPUS Corpus**: https://opus.nlpl.eu/

## Contributing

Contributions welcome! This plugin demonstrates:
- Native plugin SDK usage
- Candle ML framework integration
- Model caching patterns
- Text-to-text packet transformation

## License

- **Plugin Code**: MPL-2.0 (StreamKit Contributors)
- **OPUS-MT Models**: Apache 2.0 (Helsinki-NLP)
- **Candle**: MIT/Apache 2.0 (Hugging Face)
