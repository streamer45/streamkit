<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Matcha TTS Native Plugin

Production-ready streaming text-to-speech using Sherpa-ONNX with Matcha models.

## Features

- 🎙️ **High-quality English voice** (LJSpeech dataset - female)
- ⚡ **Real-time on CPU** (faster than real-time on 4+ cores)
- 🔄 **Streaming output** for LLM integration
- 🎵 **22 kHz high-quality audio**
- 🎛️ **Advanced controls** (noise scale, length scale)
- 🚀 **GPU acceleration support** (CUDA/TensorRT)

## Setup

### 1. Download Models (one-time, ~90 MB)

```bash
just download-matcha-models
```

This downloads `matcha-icefall-en_US-ljspeech` to `models/` directory which includes:
- model-steps-3.onnx (~80 MB) - Acoustic model (text to mel-spectrogram)
- vocos-22khz-univ.onnx (~10 MB) - Vocoder (mel to waveform)
- tokens.txt - Token vocabulary
- espeak-ng-data/ - Phoneme conversion data
- lexicon.txt - Pronunciation rules

### 2. Build Plugin

```bash
just build-plugin-native-matcha
```

### 3. Upload to Server

```bash
just upload-matcha-plugin
```

Or manually:
```bash
curl -X POST \
  -F plugin=@target/release/libmatcha.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### 4. Verify Loaded

```bash
curl http://localhost:4545/api/v1/plugins
# Should show: plugin::native::matcha
```

## Usage

### Example Pipelines

See `samples/pipelines/oneshot/matcha-tts.yml` for a complete example.

### Parameters

| Parameter | Type | Default | Description | Runtime? |
|-----------|------|---------|-------------|----------|
| `model_dir` | string | *required* | Path to Matcha model directory | ❌ |
| `speaker_id` | integer | 0 | Voice selection (0 for LJSpeech) | ✅ |
| `speed` | number | 1.0 | Speech rate multiplier (0.5-2.0) | ✅ |
| `noise_scale` | number | 0.667 | Voice variation control (0.0-1.0) | ❌ |
| `length_scale` | number | 1.0 | Duration control (0.5-2.0) | ❌ |
| `num_threads` | integer | 4 | CPU threads for inference (1-16) | ❌ |
| `min_sentence_length` | integer | 10 | Chars to buffer before TTS | ❌ |
| `execution_provider` | string | cpu | ONNX provider (cpu/cuda/tensorrt) | ❌ |

**Runtime adjustable (✅)**: Can be changed via `TuneNode` without recreating the node.
**Not runtime adjustable (❌)**: Set at engine creation time. To change, create a new node instance with different values.

### Basic Configuration

```yaml
steps:
  - kind: plugin::native::matcha
    params:
      model_dir: "models/matcha-icefall-en_US-ljspeech"
      speaker_id: 0
      speed: 1.0
      num_threads: 4
```

### Advanced Tuning

```yaml
steps:
  - kind: plugin::native::matcha
    params:
      model_dir: "models/matcha-icefall-en_US-ljspeech"
      speaker_id: 0
      speed: 1.2          # Slightly faster
      noise_scale: 0.8    # More variation
      length_scale: 0.95  # Slightly shorter duration
      num_threads: 8
```

### GPU Acceleration

```yaml
steps:
  - kind: plugin::native::matcha
    params:
      model_dir: "models/matcha-icefall-en_US-ljspeech"
      execution_provider: cuda  # or tensorrt
      num_threads: 2            # Lower threads with GPU
```

**Note**: Requires a GPU-enabled build of sherpa-onnx/ONNX Runtime (e.g. the `latest-gpu` Docker image).

## Performance

### Benchmarks

- **CPU**: ~0.5-1.5x real-time on modern CPUs (faster than playback!)
- **GPU**: ~0.05-0.1x real-time with CUDA (10-20x speedup)
- **Memory**: ~200 MB (model + runtime)
- **Latency**: 300-800ms per sentence chunk
- **Quality**: High (Matcha + Vocos vocoder)

### Expected Performance

| Hardware | RTF | Real-time? |
|----------|-----|------------|
| Intel i7 (modern, 4 cores) | ~0.8 | ✅ Yes |
| AMD Ryzen 5 (4 cores) | ~0.7 | ✅ Yes |
| Apple M1 (4 cores) | ~0.5 | ✅ Yes |
| NVIDIA RTX 4000 Ada (GPU) | ~0.05 | ✅ Yes (20x faster!) |

*RTF (Real-Time Factor): < 1.0 = faster than real-time*

## Architecture

### Streaming Model

The plugin uses **sentence-based streaming**:

1. **Text arrives** from upstream (e.g., LLM)
2. **Buffer accumulates** text until sentence boundary detected (`. ! ?`)
3. **TTS generates** audio for complete sentence
4. **Audio emitted** immediately (no waiting for full text)
5. **Process repeats** for next sentence

This balances latency vs. quality (avoids mid-word cuts).

### Data Flow

```
Text Packets → Text Buffer → Sentence Splitter → Matcha TTS → Audio Packets (22kHz)
```

## Technical Details

### Dependencies

- **sherpa-onnx** (1.12+): C API for ONNX Runtime
- **streamkit-plugin-sdk-native**: StreamKit plugin SDK
- **unicode-segmentation**: Sentence boundary detection

### Audio Output

- **Sample rate**: 22050 Hz (fixed by Matcha model)
- **Channels**: Mono (1 channel)
- **Format**: F32 samples in range [-1.0, 1.0]
- **Frame size**: Variable (depends on sentence length)

**Note**: If your pipeline requires 48kHz, add a `resampler` node after the TTS node.

### Model Attribution

- **Matcha models**: Trained using icefall framework on LJSpeech dataset
- **Source**: K2-FSA project - https://github.com/k2-fsa/sherpa-onnx
- **Documentation**: https://k2-fsa.github.io/sherpa/onnx/tts/pretrained_models/matcha.html
- **Model page**: https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models
- **License**: Apache 2.0

## Troubleshooting

### "Model files not found"

Ensure you've downloaded models:
```bash
just download-matcha-models
```

Check that `model_dir` parameter points to the correct location.

### "TTS generation failed"

- Check that models are complete (re-download if corrupted)
- Verify sufficient RAM (~500 MB available)
- Try reducing `num_threads` if CPU is overloaded
- If using GPU, ensure CUDA is properly installed

### Slow performance (RTF > 1.0)

- Increase `num_threads` (try 4-8)
- Check CPU usage (should be near 100% during generation)
- Ensure no other heavy processes running
- Consider GPU acceleration (see GPU Acceleration section)

### Audio quality issues

- Adjust `noise_scale` (lower = more consistent, higher = more natural variation)
- Tune `length_scale` (affects prosody and duration)
- Adjust `speed` parameter (too fast/slow can affect quality)
- Check sentence boundary detection isn't cutting off mid-word

### GPU initialization fails

- Verify CUDA installation: `nvidia-smi`
- Check sherpa-onnx was built with CUDA support
- Try CPU fallback by setting `execution_provider: cpu`
- See [DOCKER.md](../../../DOCKER.md) for troubleshooting

## Development

### Running Tests

```bash
cd plugins/native
cargo test
```

### Debugging

Enable logging in plugin:
```rust
plugin_info!(logger, "Debug message");
plugin_warn!(logger, "Warning message");
plugin_error!(logger, "Error message");
```

Check server logs for detailed output.

## Comparison with Other TTS Plugins

| Plugin | Voices | Sample Rate | Model Size | Special Features |
|--------|--------|-------------|------------|------------------|
| **Matcha** | 1 (female) | 22050 Hz | ~90 MB | Advanced controls (noise/length scale) |
| **Kokoro** | 103 (multi-lang) | 24000 Hz | ~360 MB | Many voices, Chinese support |
| **Piper** | 904+ | 22050 Hz | ~80 MB | Wide voice selection |

Choose Matcha when you want:
- Fine-grained control over prosody
- High-quality single voice
- GPU acceleration support
- English-only TTS
