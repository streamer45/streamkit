<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# SenseVoice STT Native Plugin

High-performance multilingual speech-to-text plugin for StreamKit using [SenseVoice](https://github.com/k2-fsa/sherpa-onnx) via sherpa-onnx.

## Features

- **Multilingual Support**: Chinese (Mandarin), Cantonese, English, Japanese, and Korean in a single model
- **Automatic Language Detection**: Detects language automatically without configuration
- **Inverse Text Normalization (ITN)**: Adds proper punctuation to transcriptions
- **VAD-Based Segmentation**: Optional Silero VAD integration for natural speech boundaries
- **Real-time Performance**: Optimized INT8 quantized model for CPU inference
- **GPU Acceleration**: Supports CUDA and TensorRT execution providers
- **Model Caching**: Automatic deduplication across pipeline instances

## Supported Languages

| Language | Code | Description |
|----------|------|-------------|
| Chinese (Mandarin) | `zh` | Simplified/Traditional Chinese |
| Cantonese | `yue` | 广东话 (Guangdonghua) |
| English | `en` | English |
| Japanese | `ja` | 日本語 |
| Korean | `ko` | 한국어 |
| Auto-detect | `auto` | Automatic language detection (default) |

## Quick Start

### 1. Install Dependencies

```bash
# Install sherpa-onnx shared library
just install-sherpa-onnx

# Download SenseVoice models and Silero VAD (one-time, ~240 MB)
just setup-sensevoice
```

### 2. Build Plugin

```bash
# Build the plugin
just build-plugin-native-sensevoice

# Upload to running server
just upload-sensevoice-plugin
```

### 3. Use in Pipeline

```yaml
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: audio::resampler
    params:
      target_sample_rate: 16000  # SenseVoice requires 16kHz
      chunk_frames: 960
  - kind: plugin::native::sensevoice
    params:
      language: auto          # Auto-detect language
      use_itn: true          # Add punctuation
      use_vad: true          # Enable VAD segmentation
  - kind: json_serialize
  - kind: streamkit::http_output
```

See `samples/pipelines/oneshot/sensevoice-stt.yml` for complete example.

## Configuration Parameters

### Model Parameters (Cached)

These parameters affect model loading and are used for caching:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | `models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09` | Path to model directory |
| `language` | string | `auto` | Language code (`auto`, `zh`, `en`, `ja`, `ko`, `yue`) |
| `num_threads` | integer | `4` | CPU threads for inference (1-16) |
| `execution_provider` | string | `cpu` | ONNX Runtime provider (`cpu`, `cuda`, `tensorrt`) |

### Processing Parameters (Per-Instance)

These parameters can differ between instances sharing the same model:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `use_itn` | boolean | `true` | Enable inverse text normalization (punctuation) |
| `use_vad` | boolean | `true` | Enable VAD-based segmentation |
| `vad_model_path` | string | `models/silero_vad.onnx` | Path to Silero VAD model |
| `vad_threshold` | number | `0.5` | Speech detection threshold (0.0-1.0) |
| `min_silence_duration_ms` | integer | `700` | Minimum silence before segmenting (ms) |
| `max_segment_duration_secs` | number | `30.0` | Maximum segment duration (seconds) |

## Audio Requirements

SenseVoice requires audio in the following format:

- **Sample rate**: 16 kHz (use `audio::resampler` to convert)
- **Channels**: Mono (1 channel)
- **Format**: f32 samples

The plugin will validate input and provide clear error messages if the format is incorrect.

## VAD Segmentation

VAD (Voice Activity Detection) segments audio into natural speech boundaries:

**With VAD enabled** (`use_vad: true`, default):
- Detects speech vs. silence using Silero VAD
- Transcribes complete sentences when silence is detected
- Zero chunking artifacts, natural boundaries
- Best for conversational audio and streaming

**With VAD disabled** (`use_vad: false`):
- Transcribes audio in fixed-duration segments
- Uses `max_segment_duration_secs` for chunking
- Best for continuous speech with minimal pauses

## Model Caching

The plugin automatically caches recognizers to avoid redundant model loading:

**Cache Key**: `(model_dir, language, num_threads, execution_provider)`

**Example**: Three pipeline instances using the same model configuration will share a single recognizer in memory.

**Cache Hits**: Logged as `✅ CACHE HIT: Reusing cached recognizer`
**Cache Misses**: Logged as `❌ CACHE MISS: Creating new recognizer`

## Model Attribution

- **SenseVoice Model**: [K2-FSA/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
- **Model Version**: `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09`
- **Model Size**: 226 MB (INT8 quantized)
- **License**: Apache 2.0
- **Training Data**: Fine-tuned on 21.8k hours of Cantonese data (improved Cantonese recognition)

## Performance

### CPU Performance (Intel i7-13700K, 4 threads)

| Language | RTF (Real-Time Factor) | Description |
|----------|------------------------|-------------|
| English | ~0.1-0.2x | 5-10x faster than real-time |
| Chinese | ~0.15-0.25x | 4-7x faster than real-time |
| Japanese | ~0.15-0.25x | 4-7x faster than real-time |
| Korean | ~0.15-0.25x | 4-7x faster than real-time |
| Cantonese | ~0.15-0.25x | 4-7x faster than real-time |

*Lower RTF is better (0.5x = 2x faster than real-time)*

### GPU Acceleration

SenseVoice supports GPU acceleration via CUDA and TensorRT execution providers.

**Docker GPU Image**: GPU support is automatically enabled when building with `Dockerfile.gpu`. The image includes sherpa-onnx with CUDA support.

**Configuration**:
```yaml
- kind: plugin::native::sensevoice
  params:
    execution_provider: cuda  # or "tensorrt" for even better performance
    num_threads: 4             # Still used for CPU preprocessing
```

**Expected Performance**:

| GPU Model | RTF (Real-Time Factor) | Speedup vs CPU |
|-----------|------------------------|----------------|
| RTX 4090 | ~0.01-0.02x | ~10-15x faster |
| RTX 4000 Ada | ~0.02-0.03x | ~8-12x faster |
| T4 | ~0.05-0.08x | ~4-6x faster |

*Lower RTF is better (0.01x = 100x faster than real-time)*

**Host Requirements** (for Docker GPU):
- ✅ NVIDIA driver (545+ for CUDA 12.x, 580 recommended)
- ✅ nvidia-container-toolkit
- ✅ GPU with compute capability 5.3+ (Maxwell or newer)

**Local GPU Build**: To enable GPU support for local builds:
1. Install sherpa-onnx with CUDA support (see [sherpa-onnx installation](https://k2-fsa.github.io/sherpa/onnx/install/index.html))
2. Ensure CUDA runtime libraries are in `LD_LIBRARY_PATH`

**Note**: The plugin respects your `execution_provider` configuration exactly as specified. If CUDA is not available and you request `cuda`, initialization will fail with an error rather than silently falling back to CPU.

## Comparison with Whisper

| Feature | SenseVoice | Whisper |
|---------|------------|---------|
| Languages | 5 languages (CN, Cantonese, EN, JA, KO) | 99 languages |
| Model Size | 226 MB (INT8) | 140 MB (base.en-q5_1) |
| CPU Performance | ~5-10x realtime | ~10-15x realtime |
| Cantonese Support | ✅ Excellent (fine-tuned) | ⚠️ Limited |
| Japanese/Korean | ✅ Native support | ✅ Good |
| Punctuation | ✅ Built-in ITN | ⚠️ Basic |
| Use Case | Asian languages, Cantonese | General multilingual |

## Troubleshooting

### Plugin fails to load

```
Error: Failed to load sherpa-onnx shared library
```

**Solution**: Install sherpa-onnx:
```bash
just install-sherpa-onnx
```

### Audio format error

```
Error: SenseVoice requires 16kHz audio, got 48000Hz
```

**Solution**: Add `audio::resampler` upstream:
```yaml
- kind: audio::resampler
  params:
    target_sample_rate: 16000
```

### Model not found

```
Error: model file not found: models/.../model.int8.onnx
```

**Solution**: Download models:
```bash
just download-sensevoice-models
```

### Poor transcription quality

Try adjusting VAD parameters:
```yaml
params:
  vad_threshold: 0.6          # Higher = more strict speech detection
  min_silence_duration_ms: 500  # Lower = more frequent segmentation
```

### GPU not being used (high CPU usage)

**Solutions**:
1. Verify Docker was started with `--gpus all`:
   ```bash
   docker run --gpus all -p 4545:4545 streamkit:gpu
   ```

2. Check NVIDIA driver and CUDA availability:
   ```bash
   nvidia-smi  # Should show your GPU
   ```

3. Ensure you're using the GPU-enabled Docker image (`Dockerfile.gpu`)

4. Check the pipeline configuration has `execution_provider: "cuda"`:
   ```yaml
   - kind: plugin::native::sensevoice
     params:
       execution_provider: cuda  # Must be explicit, not optional
   ```

**Verify GPU is active**: Look for these logs at startup:
```
Initializing recognizer with execution_provider='cuda'
✓ Recognizer created successfully
```

**Note**: If CUDA is not available or initialization fails, the plugin will return an error rather than silently falling back to CPU. Check the error message for details about what went wrong.

## Development

### Build from source

```bash
cd plugins/native
cargo build --release
```

### Run tests

```bash
cargo test
```

### Lint

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
```

## References

- [sherpa-onnx SenseVoice Documentation](https://k2-fsa.github.io/sherpa/onnx/sense-voice/index.html)
- [SenseVoice C API](https://k2-fsa.github.io/sherpa/onnx/sense-voice/c-api.html)
- [K2-FSA GitHub](https://github.com/k2-fsa/sherpa-onnx)

## License

This plugin is licensed under MPL-2.0. The SenseVoice model is licensed under Apache 2.0.
