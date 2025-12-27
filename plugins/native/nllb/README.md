<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# NLLB Translation Native Plugin

A high-performance native plugin for real-time neural machine translation using Meta's NLLB-200 (No Language Left Behind) models via CTranslate2.

## Features

- **200 Languages**: State-of-the-art translation quality across 200 languages
- **Real-Time Performance**: 400-600 tokens/second on CPU with INT8 quantization
- **Low Latency**: ~200-500ms per sentence translation
- **Memory Efficient**: 600MB runtime memory with INT8 quantized 600M model
- **Model Caching**: Automatic model sharing across pipeline instances
- **CPU & GPU Support**: Optimized for CPU with optional CUDA acceleration

## ⚠️ License Warning

**NLLB-200 models are licensed under CC-BY-NC-4.0 (Non-Commercial).**

- ✅ **Permitted**: Research, education, personal projects, non-commercial use
- ❌ **Not Permitted**: Commercial deployments, revenue-generating applications

For commercial use, consider:
- **Opus-MT models** (Apache 2.0 / MIT licensed)
- **Open-NLLB** (community effort for truly open NLLB checkpoints)
- Contact Meta AI for commercial licensing

## Quick Start

### 1. Install Dependencies

```bash
# Install Python tools for downloading from Hugging Face
pip3 install --user huggingface-hub
```

### 2. Download Pre-Converted NLLB Model

```bash
# Download pre-converted NLLB-200-distilled-600M model (~1.2 GB)
# Much faster than converting from scratch!
python3 -c "from huggingface_hub import snapshot_download; snapshot_download('entai2965/nllb-200-distilled-600M-ctranslate2', local_dir='models/nllb-200-distilled-600M-ct2-int8', local_dir_use_symlinks=False)"

# Or use the justfile command:
just download-nllb-models
```

**Alternative: Convert From Scratch** (if you need a different quantization):
```bash
# Install conversion tools
pip3 install --user ctranslate2 transformers torch

# Convert NLLB-200-distilled-600M to CTranslate2 format
ct2-transformers-converter \
  --model facebook/nllb-200-distilled-600M \
  --output_dir models/nllb-200-distilled-600M-ct2-int8 \
  --quantization int8
```

**Model Variants:**

| Model | Parameters | Disk Size | Memory (INT8) | Quality | Speed | Use Case |
|-------|-----------|-----------|---------------|---------|-------|----------|
| NLLB-200-distilled-600M | 600M | ~1.2 GB | ~600 MB | Good | Fast | ✅ **Recommended** for real-time |
| NLLB-200-distilled-1.3B | 1.3B | ~2.6 GB | ~1.5 GB | Better | Medium | Balanced quality/speed |
| NLLB-200-3.3B | 3.3B | ~6.6 GB | ~2.5 GB | Best | Slow | High-quality offline processing |

**Quantization Options:**

```bash
# INT8 (recommended for CPU, 3-4x faster than float32)
--quantization int8

# INT16 (balanced)
--quantization int16

# Float16 (GPU-friendly)
--quantization float16

# Float32 (highest quality, slowest)
# Omit --quantization flag
```

### 3. Build the Plugin

```bash
# From repository root
just build-plugin-native-nllb

# Or manually
cd plugins/native
cargo build --release

# Plugin binary:
# Linux: target/release/libnllb.so
# macOS: target/release/libnllb.dylib
# Windows: target/release/nllb.dll
```

### 4. Upload to StreamKit Server

```bash
# Start server
just skit serve

# Upload plugin (in another terminal)
just upload-nllb-plugin

# Or manually
curl -X POST \
  -F plugin=@plugins/native/nllb/target/release/libnllb.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### 5. Use in Pipelines

The plugin will appear as `plugin::native::nllb` in the node registry.

**Simple Translation Pipeline:**

```yaml
steps:
  - kind: streamkit::http_input

  - kind: plugin::native::nllb
    params:
      model_path: models/nllb-200-distilled-600M-ct2-int8
      source_language: eng_Latn    # English
      target_language: spa_Latn    # Spanish
      compute_type: int8
      beam_size: 1                 # Greedy (fast)
      num_threads: 4

  - kind: streamkit::http_output
```

**Complete Example Pipelines:**

- `samples/pipelines/oneshot/speech_to_text_translate.yml` - Audio → Whisper → NLLB → JSON
- `samples/pipelines/oneshot/translate_to_speech.yml` - Audio → Whisper → NLLB → Kokoro TTS → Audio

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_path` | string | `models/nllb-200-distilled-600M-ct2-int8` | Path to CTranslate2 model directory |
| `source_language` | string | `eng_Latn` | Source language code (NLLB format, see below) |
| `target_language` | string | `spa_Latn` | Target language code (NLLB format, see below) |
| `compute_type` | string | `int8` | Precision: `auto`, `int8`, `int16`, `float16`, `float32` |
| `beam_size` | integer | `1` | Beam search size (1 = greedy/fast, 4 = better quality/slower) |
| `num_threads` | integer | `0` | Number of threads (0 = auto, recommend 4-8 for real-time) |
| `device` | string | `cpu` | Device: `cpu`, `cuda`, or `auto` |
| `device_index` | integer | `0` | GPU device index (only used when device is `cuda`) |

## Language Codes

NLLB uses BCP-47 language codes with script variants. Format: `{language}_{Script}`

### Common Languages

| Language | Code | Language | Code |
|----------|------|----------|------|
| **English** | `eng_Latn` | **Spanish** | `spa_Latn` |
| **French** | `fra_Latn` | **German** | `deu_Latn` |
| **Italian** | `ita_Latn` | **Portuguese** | `por_Latn` |
| **Russian** | `rus_Cyrl` | **Arabic** | `arb_Arab` |
| **Chinese (Simplified)** | `zho_Hans` | **Chinese (Traditional)** | `zho_Hant` |
| **Japanese** | `jpn_Jpan` | **Korean** | `kor_Hang` |
| **Hindi** | `hin_Deva` | **Bengali** | `ben_Beng` |
| **Turkish** | `tur_Latn` | **Polish** | `pol_Latn` |
| **Dutch** | `nld_Latn` | **Swedish** | `swe_Latn` |
| **Vietnamese** | `vie_Latn` | **Thai** | `tha_Thai` |
| **Indonesian** | `ind_Latn` | **Malay** | `zsm_Latn` |
| **Hebrew** | `heb_Hebr` | **Greek** | `ell_Grek` |

### Script Codes

- `Latn` - Latin alphabet (English, Spanish, French, etc.)
- `Cyrl` - Cyrillic (Russian, Ukrainian, etc.)
- `Arab` - Arabic script
- `Hans` - Simplified Chinese characters
- `Hant` - Traditional Chinese characters
- `Jpan` - Japanese (Kanji + Kana)
- `Hang` - Korean Hangul
- `Deva` - Devanagari (Hindi, Sanskrit, etc.)
- `Beng` - Bengali script
- `Thai` - Thai script
- `Hebr` - Hebrew script
- `Grek` - Greek alphabet

**Complete List**: See [NLLB-200 Supported Languages](https://github.com/facebookresearch/flores/blob/main/flores200/README.md#languages-in-flores-200) (200+ languages)

## Performance Tuning

### Real-Time Translation (Low Latency)

```yaml
params:
  compute_type: int8              # Fastest
  beam_size: 1                    # Greedy decoding
  num_threads: 4                  # 4-8 threads optimal for most CPUs
```

**Expected Performance:**
- Latency: ~200-300ms per sentence (10-20 words)
- Throughput: ~400-600 tokens/second
- Memory: ~600 MB

### High-Quality Translation (Offline Processing)

```yaml
params:
  compute_type: float16           # Better precision
  beam_size: 4                    # Beam search for quality
  num_threads: 8                  # More threads for throughput
```

**Expected Performance:**
- Latency: ~500-1000ms per sentence
- Throughput: ~200-300 tokens/second
- Memory: ~1-2 GB

### GPU Acceleration

**Docker GPU Image**: GPU support is automatically enabled when building with `Dockerfile.gpu`. The image includes CTranslate2 with CUDA support.

```yaml
params:
  device: cuda
  device_index: 0
  compute_type: float16           # GPU-optimized
  beam_size: 1
```

**Expected Performance:**
- Latency: ~50-100ms per sentence (5-10x faster than CPU)
- Throughput: ~2000-5000 tokens/second

**Local GPU Build**: To enable GPU support for local builds, you need:
1. CTranslate2 compiled with CUDA support (see [CTranslate2 installation](https://opennmt.net/CTranslate2/installation.html))
2. Modify `Cargo.toml` to enable CUDA features:
   ```toml
   ct2rs = { version = "0.9", features = ["cuda", "cudnn"] }
   ```

## Model Caching

The plugin automatically caches models across pipeline instances to save memory and reduce startup time.

**Cache Key**: `(model_path, compute_type, device, device_index)`

**Example**: Two pipeline instances with the same model path and compute type will share the same model in memory.

**Cache Behavior:**
- ✅ **Shared**: model_path, compute_type, device settings
- ❌ **Not Shared**: source_language, target_language, beam_size (per-instance parameters)

## Troubleshooting

### Build Errors

**CMake Not Found:**
```bash
# Ubuntu/Debian
sudo apt-get install cmake

# macOS
brew install cmake
```

**CTranslate2 Compilation Fails:**
```bash
# Ensure you have a C++17 compiler
# Ubuntu/Debian
sudo apt-get install build-essential

# macOS
xcode-select --install
```

### Runtime Errors

**Model Not Found:**
```
Failed to load NLLB model from 'models/nllb-200-distilled-600M-ct2-int8'
```

**Solution**: Verify model path and ensure conversion completed successfully.

**Unsupported Language Code:**
```
Translation failed: unsupported language code
```

**Solution**: Check language codes against NLLB-200 supported languages. Use format `{lang}_{Script}`.

**Out of Memory:**
```
Failed to allocate memory for model
```

**Solution**: Use INT8 quantization or a smaller model (600M instead of 1.3B/3.3B).

## Architecture

### Model Loading Flow

1. Parse configuration from pipeline YAML
2. Compute cache key: `(model_path, compute_type, device, device_index)`
3. Check global cache (`TRANSLATOR_CACHE`)
4. If cache miss: Load model from disk → Create `TranslatorPool` → Cache it
5. If cache hit: Reuse existing `Arc<TranslatorPool>`

### Translation Flow

1. Receive `Packet::Text` from upstream node (e.g., Whisper STT)
2. Skip empty text
3. Get translator from pool
4. Prepare input with source language token
5. Translate with target language prefix
6. Extract first hypothesis (best translation)
7. Emit `Packet::Text` to downstream node

### Thread Safety

- Model cache: Protected by `Mutex`
- Translator pool: Thread-safe internal queue
- Each translation request is independent

## Performance Benchmarks

**Test System**: Intel Xeon Platinum 8275CL (4 threads), NLLB-600M-INT8

| Sentence Length | Latency | Tokens/sec | Memory |
|----------------|---------|------------|--------|
| 10 words | ~200ms | 500 | 600 MB |
| 20 words | ~350ms | 570 | 600 MB |
| 30 words | ~500ms | 600 | 600 MB |
| 50 words | ~750ms | 665 | 600 MB |

**Note**: Batch processing (multiple sentences) can achieve 10x+ higher throughput.

## Comparison with Alternatives

| Solution | Languages | Speed (CPU) | License | Integration |
|----------|-----------|-------------|---------|-------------|
| **NLLB-200** | 200 | ⭐⭐⭐⭐ | ❌ Non-commercial | ✅ Easy (this plugin) |
| Opus-MT | 2 per model | ⭐⭐⭐⭐ | ✅ Apache 2.0 | ✅ Easy (via ct2rs) |
| Google Translate API | 100+ | ⭐⭐⭐⭐⭐ | 💰 Commercial | ⚠️ API dependency |
| LibreTranslate | 30+ | ⭐⭐⭐ | ✅ AGPL 3.0 | ⚠️ HTTP API overhead |

## References

- **NLLB-200 Paper**: https://arxiv.org/abs/2207.04672
- **CTranslate2**: https://github.com/OpenNMT/CTranslate2
- **ct2rs**: https://github.com/jkawamoto/ctranslate2-rs
- **NLLB Models**: https://huggingface.co/facebook/nllb-200-distilled-600M
- **Language Codes**: https://github.com/facebookresearch/flores/blob/main/flores200/README.md

## Contributing

Contributions welcome! This plugin demonstrates:
- Native plugin SDK usage
- CTranslate2 integration
- Model caching patterns
- Text-to-text packet transformation

## License

- **Plugin Code**: MPL-2.0 (StreamKit Contributors)
- **NLLB-200 Models**: CC-BY-NC-4.0 (Meta AI)
- **CTranslate2**: MIT (OpenNMT)
- **ct2rs**: Apache 2.0 (Jun Kawamoto)
