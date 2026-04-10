<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Parakeet TDT STT Native Plugin

High-performance English speech-to-text plugin for StreamKit using [NVIDIA Parakeet TDT](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2) via sherpa-onnx.

## Features

- **Best-in-class Accuracy**: #1 on HuggingFace ASR leaderboard, lower WER than Whisper
- **Fast CPU Inference**: ~10x faster than Whisper Large V3 on CPU (35 min audio in ~18s on Apple Silicon)
- **INT8 Quantized**: 631 MB model runs well on consumer hardware
- **VAD-Based Segmentation**: Optional Silero VAD integration for natural speech boundaries
- **GPU Acceleration**: Supports CUDA and TensorRT execution providers
- **Model Caching**: Automatic deduplication across pipeline instances
- **Commercially Permissive**: CC-BY-4.0 license

## Quick Start

### 1. Install Dependencies

```bash
# Install sherpa-onnx shared library
just install-sherpa-onnx

# Download Parakeet models and Silero VAD (one-time, ~631 MB)
just setup-parakeet
```

### 2. Build Plugin

```bash
# Build the plugin
just build-plugin-native-parakeet

# Copy to plugins directory
just copy-plugins-native

# Or upload to running server
just upload-parakeet-plugin
```

### 3. Use in Pipeline

```yaml
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: audio::resampler
    params:
      target_sample_rate: 16000  # Parakeet requires 16kHz
      chunk_frames: 960
  - kind: plugin::native::parakeet
    params:
      use_vad: true           # Enable VAD segmentation
      num_threads: 4          # CPU threads for inference
  - kind: core::json_serialize
  - kind: streamkit::http_output
```

See `samples/pipelines/oneshot/parakeet-stt.yml` for complete example.

## Configuration Parameters

### Model Parameters (Cached)

These parameters affect model loading and are used for caching:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | `models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8` | Path to model directory |
| `num_threads` | integer | `4` | CPU threads for inference (1-16) |
| `execution_provider` | string | `cpu` | ONNX Runtime provider (`cpu`, `cuda`, `tensorrt`) |

### Processing Parameters (Per-Instance)

These parameters can differ between instances sharing the same model:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `use_vad` | boolean | `true` | Enable VAD-based segmentation |
| `vad_model_path` | string | `models/silero_vad.onnx` | Path to Silero VAD model |
| `vad_threshold` | number | `0.5` | Speech detection threshold (0.0-1.0) |
| `min_silence_duration_ms` | integer | `700` | Minimum silence before segmenting (ms) |
| `max_segment_duration_secs` | number | `30.0` | Maximum segment duration (seconds) |

## Audio Requirements

Parakeet requires audio in the following format:

- **Sample rate**: 16 kHz (use `audio::resampler` to convert)
- **Channels**: Mono (1 channel)
- **Format**: f32 samples

## Model Architecture

Parakeet TDT uses a **Token-and-Duration Transducer** architecture with three ONNX models:

| File | Size | Description |
|------|------|-------------|
| `encoder.int8.onnx` | ~652 MB | FastConformer encoder (INT8 quantized) |
| `decoder.int8.onnx` | ~7 MB | Prediction network |
| `joiner.int8.onnx` | ~2 MB | Joint network |
| `tokens.txt` | ~9 KB | Token vocabulary |

This contrasts with single-model architectures (SenseVoice, Whisper) — the transducer approach enables faster streaming-friendly decoding.

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

**Cache Key**: `(model_dir, num_threads, execution_provider)`

Multiple pipeline instances using the same model configuration share a single recognizer in memory.

## Comparison with Other STT Plugins

| Feature | Parakeet TDT | SenseVoice | Whisper |
|---------|-------------|------------|---------|
| Languages | English only (v2) | 5 languages | 99 languages |
| Model Size | 631 MB (INT8) | 226 MB (INT8) | 140 MB (base.en-q5_1) |
| CPU Speed | ~10x faster than Whisper | ~5-10x realtime | ~10-15x realtime |
| Accuracy (WER) | Best (#1 HF leaderboard) | Good | Good |
| Architecture | Transducer (enc/dec/joiner) | Single model | Single model |
| License | CC-BY-4.0 | Apache 2.0 | MIT |
| Best For | Fast, accurate English STT | Asian languages | General multilingual |

## Troubleshooting

### Plugin fails to load

```
Error: Failed to load sherpa-onnx shared library
```

**Solution**: Install sherpa-onnx:
```bash
just install-sherpa-onnx
```

### Model not found

```
Error: model file not found: models/.../encoder.int8.onnx
```

**Solution**: Download models:
```bash
just download-parakeet-models
```

### Audio format error

**Solution**: Add `audio::resampler` upstream:
```yaml
- kind: audio::resampler
  params:
    target_sample_rate: 16000
```

## Model Attribution

- **Parakeet TDT Model**: [NVIDIA NeMo](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2)
- **sherpa-onnx Export**: [csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8](https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8)
- **Silero VAD**: [snakers4/silero-vad](https://github.com/snakers4/silero-vad) (MIT)
- **License**: CC-BY-4.0

## License

This plugin is licensed under MPL-2.0. The Parakeet TDT model is licensed under CC-BY-4.0.
