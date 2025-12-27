<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Whisper Native Plugin with VAD

A high-performance native plugin for real-time speech-to-text transcription using [Whisper](https://github.com/openai/whisper) via [whisper.cpp](https://github.com/ggerganov/whisper.cpp) bindings with Silero VAD for intelligent speech segmentation.

## Features

- **VAD-based segmentation**: Uses Silero VAD v6 to detect natural speech boundaries
- **Zero chunking artifacts**: No duplicate text or mid-sentence cutoffs
- **Real-time performance**: Achieves 10-30x real-time speed on CPU with base.en model
- **Streaming support**: Processes audio continuously with automatic silence detection
- **Structured output**: Returns transcription data with segment-level timestamps
- **Native performance**: ~0-5% overhead compared to built-in nodes
- **Composable**: Requires 16kHz mono input (use `audio_resample` node upstream)
- **Configurable**: Model paths, VAD threshold, silence duration all configurable at runtime

## How It Works

The plugin uses a two-stage architecture:

1. **VAD Stage**: Silero VAD v6 processes audio in 32ms frames (512 samples @ 16kHz)
   - Detects speech vs. silence in real-time
   - Maintains LSTM state for temporal context
   - Configurable threshold (0.0-1.0, default 0.5)

2. **Transcription Stage**: Whisper transcribes complete speech segments
   - Segments triggered by silence detection (default 700ms)
   - Max segment duration prevents run-on speech (default 30s)
   - Natural boundaries = better accuracy, no deduplication needed

## Node Identity

- **Kind**: `plugin::native::whisper`
- **Categories**: `ml`, `speech`, `transcription`

## Input/Output

### Input Pin: `in`
- **Accepts**: `Packet::Audio` (f32 samples, **MUST be 16kHz mono**)
- **Validation**: Plugin will error if audio is not 16kHz mono
- **Composability**: Use `audio_resample` node upstream to convert formats

### Output Pin: `out`
- **Produces**: `Packet::Transcription` (structured transcription data)
- **Includes**:
  - Full transcribed text
  - Individual segments with absolute timestamps (milliseconds)
  - Language information
- **Timing**: Transcription emitted when speech segment ends (silence detected or max duration reached)

## Configuration Parameters

```yaml
model_path: "models/ggml-base.en-q5_1.bin" # Path to Whisper GGML model
language: "en"                              # Language code (en, es, fr, etc.)
vad_model_path: "models/silero_vad.onnx"   # Path to Silero VAD model
vad_threshold: 0.5                          # Speech probability threshold (0.0-1.0)
min_silence_duration_ms: 700                # Silence duration before segment ends (ms)
max_segment_duration_secs: 30.0             # Force segment after this duration (seconds)
n_threads: 0                                # Number of threads for decoding (0 = auto)
```

## Building

### Prerequisites

1. **Rust toolchain**: Install from [rustup.rs](https://rustup.rs/)
2. **Whisper model files**: Download GGML models (see [Model Setup](#model-setup))
3. **Silero VAD model**: Download ONNX model (see [VAD Model Setup](#vad-model-setup))

### Build Commands

```bash
# Build the plugin
just build-plugin-native whisper

# Or build directly
cd plugins/native
cargo build --release

# The plugin will be at:
# target/release/libwhisper.so    (Linux)
# target/release/libwhisper.dylib (macOS)
# target/release/whisper.dll      (Windows)
```

### Install to plugins directory

```bash
# Build and install all plugins
just install-plugins

# The plugin will be copied to: .plugins/native/libwhisper.*
```

## Model Setup

### Whisper Models

Whisper requires GGML model files. Download them from the [official repository](https://huggingface.co/ggerganov/whisper.cpp/tree/main):

```bash
# Create models directory in repo root
mkdir -p models

# Download base.en-q5_1 model (recommended, 60MB, quantized for faster performance)
curl -L -o models/ggml-base.en-q5_1.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin

# Or download other models:
# Full precision base.en (148MB, slightly better quality)
curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin

# Q8 quantization (82MB, better quality than q5_1)
curl -L -o models/ggml-base.en-q8_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q8_0.bin

# tiny.en (75MB, fastest, ~40x realtime)
curl -L -o models/ggml-tiny.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin

# small.en (466MB, higher accuracy, ~2-10x realtime)
curl -L -o models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin
```

### VAD Model Setup

Download the Silero VAD v6 ONNX model:

```bash
# Download Silero VAD model (3.5MB)
curl -L -o models/silero_vad.onnx \
  https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx
```

**Note**: The VAD model is required for the plugin to function. Without it, initialization will fail.

## Usage

### Loading the Plugin

```bash
# Upload plugin to running skit server
curl -X POST -F plugin=@target/release/libwhisper.so \
  http://127.0.0.1:4545/api/v1/plugins

# List loaded plugins
curl http://127.0.0.1:4545/api/v1/plugins

# The plugin appears as: plugin::native::whisper
```

### Pipeline Example

See `samples/pipelines/oneshot/speech_to_text.yml` for an end-to-end oneshot pipeline (HTTP multipart input → OGG/Opus decode → resample → Whisper → JSON).

**Important**: Whisper requires **16kHz mono** audio; resample upstream accordingly.

## Performance

### Expected Performance (base.en model)

| CPU | Real-time Factor | Latency |
|-----|------------------|---------|
| Apple M2 Pro | ~30x | 0.7-5s (depends on speech patterns) |
| Intel i7 (modern) | ~10-15x | 0.7-5s |
| Intel i7 (2015) | ~1.5-3x | 1-10s |

**Real-time factor**: How much faster than real-time the processing runs. Higher is better.
- 1x = processes audio at the same speed it plays (minimum for real-time)
- 10x = processes 10 seconds of audio in 1 second

### Latency Characteristics

VAD-based segmentation provides variable latency based on natural speech:

- **Minimum latency**: 700ms (min_silence_duration_ms)
- **Typical latency**: 1-5 seconds (natural pauses in speech)
- **Maximum latency**: 30 seconds (max_segment_duration_secs)

This is much better than fixed chunking, which causes:
- Duplicate text at chunk boundaries
- Mid-sentence cutoffs
- Need for complex deduplication logic

### Model Comparison

| Model | Size | CPU Speed | Quality | Use Case |
|-------|------|-----------|---------|----------|
| tiny.en | 75MB | ~40x | Good | Demos, low-resource, ultra-low latency |
| base.en-q5_1 | 60MB | ~15-45x | Better | **Recommended** - Quantized for speed |
| base.en-q8_0 | 82MB | ~12-35x | Better+ | Higher quality quantization |
| base.en | 148MB | ~10-30x | Best | Full precision |
| small.en | 466MB | ~2-10x | Best+ | High-quality, latency tolerant |

## Tuning Parameters

### Number of Threads (`n_threads`)

Controls CPU thread usage for Whisper decoding:

- **Auto (0)**: **Recommended** - Uses whisper.cpp default: min(4, num_cores)
- **Low (1-4)**: Lower CPU usage, slower transcription
- **Medium (4-8)**: Balanced performance for most systems
- **High (8-12)**: Maximum performance on modern CPUs (M2/M3, i7/i9)
- **Very High (12+)**: Diminishing returns, may hurt performance

**Performance Notes**:
- Default (0) uses only 4 threads even on high-core CPUs
- Increasing to 8-12 threads can significantly speed up transcription on modern hardware
- Beyond ~12 threads, whisper.cpp has diminishing returns for base/small models
- For multiple concurrent pipelines, reduce threads per instance to avoid oversubscription
- Thread overhead varies by model: tiny.en benefits less from high thread counts than small.en

### VAD Threshold (`vad_threshold`)

Controls speech detection sensitivity:

- **Lower (0.3-0.4)**: More sensitive, may trigger on noise
- **Medium (0.5)**: **Recommended** - Good balance
- **Higher (0.6-0.7)**: Less sensitive, may miss quiet speech

### Minimum Silence Duration (`min_silence_duration_ms`)

How long to wait before ending a segment:

- **Shorter (300-500ms)**: Lower latency, may cut off mid-sentence
- **Medium (700ms)**: **Recommended** - Tolerates natural pauses
- **Longer (1000-2000ms)**: Better for speech with long pauses

### Maximum Segment Duration (`max_segment_duration_secs`)

Forces segmentation for long speech:

- **Shorter (15-20s)**: Better for continuous speech, more frequent updates
- **Medium (30s)**: **Recommended** - Whisper's optimal range
- **Longer (60-120s)**: May cause memory issues, poor accuracy

## Troubleshooting

### Plugin fails to load

```
Error: Failed to load VAD model from 'models/silero_vad.onnx'
```

**Solution**: Ensure both model files exist:
```bash
ls -lh models/ggml-base.en-q5_1.bin models/silero_vad.onnx
```

Download missing models using the commands in [Model Setup](#model-setup).

### VAD not detecting speech

**Symptoms**: No transcription output, or very delayed output

**Solutions**:
1. Lower VAD threshold: Try 0.3-0.4 instead of 0.5
2. Check audio levels: Ensure audio is not too quiet
3. Verify audio format: Confirm audio is 16kHz mono f32

### Speech cut off mid-sentence

**Symptoms**: Incomplete sentences in output

**Solutions**:
1. Increase `min_silence_duration_ms` to 1000-1500ms
2. Lower `vad_threshold` to 0.4 to be more sensitive

### Poor transcription quality

1. **Use larger model**: Try `ggml-small.en.bin` for better accuracy
2. **Check audio quality**: Low-quality input = low-quality output
3. **Verify audio format**: Ensure audio is correctly resampled to 16kHz mono
4. **Check VAD settings**: Ensure speech segments aren't too short

### Slow performance

1. **Use smaller model**: Try `ggml-tiny.en.bin` instead of base/small
2. **Check CPU usage**: Ensure other processes aren't competing
3. **Verify VAD overhead**: VAD adds <1ms per 32ms frame (~3% overhead)

### Memory usage

Per plugin instance:

- **Frame buffer**: ~2 KB (fixed)
- **Speech buffer**: ~2 MB max (30s @ 16kHz)
- **VAD states**: ~4 KB (fixed)
- **Whisper model**: 75-500MB (depends on model)

**Total**: ~80-510 MB per instance

## Benefits Over Chunking Approach

The previous version used fixed-size chunks with overlap and deduplication. The VAD-based approach eliminates these problems:

| Issue | Chunking | VAD-based |
|-------|----------|-----------|
| Duplicate text | Yes, requires dedup | No, segments processed once |
| Mid-sentence cuts | Yes, at chunk boundaries | No, natural boundaries |
| Latency | Fixed (chunk_duration + processing) | Variable (0.7-5s typical) |
| Code complexity | ~400 lines with dedup logic | ~300 lines, simpler |
| Accuracy | Reduced context at boundaries | Full context per segment |

## Architecture Details

### Memory Bounds

- **Frame buffer**: Fixed 512 samples = 2 KB
- **Speech buffer**: Max 30s × 16000 × 4 bytes = 1.9 MB
- **VAD states**: 2 × 1 × 128 × 4 bytes × 2 = 4 KB
- **Total max**: ~2 MB per segment

### VAD Performance

- **Model size**: 3.5 MB ONNX
- **Inference time**: <1ms per 32ms frame
- **Overhead**: ~3% of real-time (negligible)

### Processing Flow

```
Audio (16kHz mono f32)
    ↓
[Frame Buffer] → Accumulates to 512 samples
    ↓
[Silero VAD] → Speech/silence detection (32ms frames)
    ↓
[Speech Buffer] → Accumulates during speech
    ↓
[Silence Detected] → Triggers transcription
    ↓
[Whisper] → Transcribes complete segment
    ↓
[Output] → Emits TranscriptionData
```

## License

This plugin is part of StreamKit and is licensed under MPL-2.0.

The Whisper model weights are released by OpenAI under the MIT license.

The Silero VAD model is released under the MIT license by Silero Team.
