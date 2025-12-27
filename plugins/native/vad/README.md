<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Voice Activity Detection (VAD) Native Plugin

A high-performance native plugin for real-time voice activity detection using [ten-vad](https://github.com/TEN-framework/ten-vad) via [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

## Features

- **Real-time VAD**: Frame-level speech detection with low latency
- **Two Output Modes**:
  - **Events mode**: Emits JSON events on speech start/stop transitions
  - **Filtered audio mode**: Passes through only audio segments containing speech
- **High accuracy**: Superior precision-recall performance compared to WebRTC VAD and Silero VAD
- **Lightweight**: ~300KB model, minimal computational overhead (RTF 0.005-0.057)
- **Zero-copy performance**: Native plugin with ~0-5% overhead vs built-in nodes
- **Model caching**: Automatic sharing of VAD detector across multiple pipeline instances

## Requirements

### System Dependencies

- **sherpa-onnx** C library (v1.10.27 or later)
- **libsherpa-onnx-c-api.so** must be installed in `/usr/local/lib`

### Installation

```bash
# Install sherpa-onnx (one-time setup)
just install-sherpa-onnx

# Download ten-vad model (~324KB)
just download-tenvad-models

# Or do both at once
just setup-vad
```

This will:
1. Install sherpa-onnx v1.10.27 to `/usr/local/lib`
2. Download `models/ten-vad.onnx` (~324KB)

## Building

```bash
# Build the plugin
just build-plugin-native-vad

# Build and upload to running server
just upload-vad-plugin
```

The plugin binary will be at:
- **Linux**: `target/release/libvad.so`
- **macOS**: `target/release/libvad.dylib`
- **Windows**: `target/release/vad.dll`

## Usage

The VAD plugin is registered as `plugin::native::vad`.

### Events mode

Use the built-in oneshot sample pipeline:

- `samples/pipelines/oneshot/vad-demo.yml`

Run it:

```bash
curl -X POST http://127.0.0.1:4545/api/v1/process \
  -F config=@samples/pipelines/oneshot/vad-demo.yml \
  -F media=@samples/audio/system/sample.ogg
```

### Filtered-audio mode (VAD → STT)

Use the built-in oneshot sample pipeline:

- `samples/pipelines/oneshot/vad-filtered-stt.yml`

Run it:

```bash
curl -X POST http://127.0.0.1:4545/api/v1/process \
  -F config=@samples/pipelines/oneshot/vad-filtered-stt.yml \
  -F media=@samples/audio/system/sample.ogg
```

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_path` | string | `models/ten-vad.onnx` | Path to ten-vad ONNX model |
| `output_mode` | enum | `events` | Output mode: `events` or `filtered_audio` |
| `threshold` | float | `0.5` | VAD threshold (0.0-1.0). Higher = more conservative |
| `min_silence_duration_s` | float | `0.5` | Minimum silence duration (seconds) to trigger speech end |
| `min_speech_duration_s` | float | `0.25` | Minimum speech duration (seconds) to be considered valid |
| `window_size` | int | `512` | Window size for VAD processing (samples) |
| `max_speech_duration_s` | float | `30.0` | Maximum speech duration (seconds) before auto-split |
| `num_threads` | int | `1` | Number of threads for ONNX runtime |
| `provider` | string | `cpu` | ONNX execution provider (`cpu`, `cuda`, etc.) |
| `debug` | bool | `false` | Enable debug logging from sherpa-onnx |

### Tunable Parameters

Parameters that can be changed at runtime via `TuneNode`:
- `threshold`
- `min_silence_duration_s`
- `min_speech_duration_s`
- `max_speech_duration_s`
- `output_mode`
- `debug`

**Cannot be changed at runtime** (require node recreation):
- `model_path`
- `num_threads`
- `provider`

## Audio Requirements

- **Sample rate**: 16 kHz (enforced)
- **Channels**: Mono (1 channel, enforced)
- **Format**: f32 samples (StreamKit internal format)

The plugin will return an error if audio does not meet these requirements.

## Performance

- **Model size**: 324 KB (ten-vad.onnx)
- **Computational overhead**: ~0-5% vs built-in nodes
- **Real-time factor**: 0.005-0.057 (50-200x faster than realtime)
- **Latency**: ~32ms processing windows at 16kHz

**Model caching**: The VAD detector is cached and shared across multiple instances with the same `model_path`, `num_threads`, and `provider`.

## Integration Examples

See the runnable oneshot samples:

- `samples/pipelines/oneshot/vad-demo.yml` (events)
- `samples/pipelines/oneshot/vad-filtered-stt.yml` (filtered audio → Whisper)

## Troubleshooting

### "Failed to create VAD detector"

**Cause**: sherpa-onnx library not installed or model file not found.

**Solution**:
```bash
just install-sherpa-onnx
just download-tenvad-models
```

### "cannot open shared object file: libsherpa-onnx-c-api.so"

**Cause**: Linker cannot find sherpa-onnx library at runtime.

**Solution**:
```bash
# Add /usr/local/lib to library path
export LD_LIBRARY_PATH=/usr/local/lib:$LD_LIBRARY_PATH

# Or run ldconfig
sudo ldconfig
```

### "VAD requires 16kHz audio, got XXXHz"

**Cause**: Input audio is not 16kHz.

**Solution**: Add a resampler node before VAD (if available) or ensure input is 16kHz.

## License

### Plugin Code

This plugin is licensed under MPL-2.0.

### ten-vad Model

The ten-vad model uses a modified Apache License 2.0. Please review the license at:
- https://github.com/TEN-framework/ten-vad

**Important**: Review the license terms before commercial use.

### sherpa-onnx

sherpa-onnx is licensed under Apache License 2.0:
- https://github.com/k2-fsa/sherpa-onnx

## References

- **ten-vad**: https://github.com/TEN-framework/ten-vad
- **sherpa-onnx**: https://github.com/k2-fsa/sherpa-onnx
- **sherpa-onnx VAD docs**: https://k2-fsa.github.io/sherpa/onnx/vad/ten-vad.html

## Model Attribution

- **ten-vad** model by TEN-framework
- Integrated via **sherpa-onnx** by K2-FSA project
- Model repository: https://huggingface.co/k2-fsa/sherpa-onnx-vad
