<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Kokoro TTS Native Plugin

Production-ready streaming text-to-speech using Sherpa-ONNX with Kokoro models.

## Features

- 🎙️ **103 high-quality voices** (Chinese + English)
- ⚡ **Real-time on CPU** (faster than real-time on 4+ cores)
- 🔄 **Streaming output** for LLM integration
- 🎵 **24 kHz high-quality audio**
- 🦀 **Safe Rust implementation** (sherpa-rs)

## Setup

### 1. Download Models (one-time, ~360 MB)

```bash
just download-kokoro-models
```

This downloads `kokoro-multi-lang-v1_1` to `models/` directory which includes:
- model.onnx (310 MB) - Main TTS model
- voices.bin (52 MB) - 103 speaker embeddings
- tokens.txt - Token vocabulary
- espeak-ng-data/ - Phoneme conversion data
- dict/ - Dictionary for Chinese (jieba)
- lexicon files - Pronunciation rules

### 2. Build Plugin

```bash
just build-plugin-native-kokoro
```

### 3. Upload to Server

```bash
just upload-kokoro-plugin
```

Or manually:
```bash
curl -X POST \
  -F plugin=@target/release/libkokoro.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### 4. Verify Loaded

```bash
curl http://localhost:4545/api/v1/plugins
# Should show: plugin::native::kokoro
```

## Usage

### Example Pipelines

See `samples/pipelines/oneshot/kokoro-tts.yml` for a complete oneshot example.

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | *required* | Path to Kokoro model directory |
| `speaker_id` | integer | 50 | Voice selection (0-102) |
| `speed` | number | 1.0 | Speech rate (0.5-2.0) |
| `num_threads` | integer | 4 | CPU threads for inference (1-16) |
| `min_sentence_length` | integer | 10 | Chars to buffer before TTS |
| `execution_provider` | string | cpu | ONNX Runtime provider (`cpu`, `cuda`, `tensorrt`) |

### Voice Selection

Try different `speaker_id` values (0-102) for variety:
- **0-52**: English voices (v1.0 compatibility)
- **53-102**: Additional voices (v1.1)

Example:
```yaml
steps:
  - kind: plugin::native::kokoro
    params:
      model_dir: "models/kokoro-multi-lang-v1_1"
      speaker_id: 75  # Try different voices!
      speed: 1.2      # Speak a bit faster
```

## Performance

### Benchmarks

- **CPU**: ~0.5-1.5x real-time on modern CPUs (faster than playback!)
- **Memory**: ~600 MB (model + runtime)
- **Latency**: 500ms-1s per sentence chunk
- **Quality**: High (82M parameter model)

### Expected Performance

| CPU | Cores | RTF | Real-time? |
|-----|-------|-----|------------|
| Intel i7 (modern) | 4 | ~0.8 | ✅ Yes |
| AMD Ryzen 5 | 4 | ~0.7 | ✅ Yes |
| Apple M1 | 4 | ~0.5 | ✅ Yes |
| Raspberry Pi 4 | 4 | ~3.0 | ❌ No |

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
Text Packets → Text Buffer → Sentence Splitter → TTS Engine → Audio Packets (24kHz)
```

## Technical Details

### Dependencies

- **sherpa-rs** (0.6+): Rust bindings to sherpa-onnx
- **streamkit-plugin-sdk-native**: StreamKit plugin SDK
- **unicode-segmentation**: Sentence boundary detection

### Audio Output

- **Sample rate**: 24000 Hz (fixed by Kokoro)
- **Channels**: Mono (1 channel)
- **Format**: F32 samples in range [-1.0, 1.0]
- **Frame size**: Variable (depends on sentence length)

**Note**: If your pipeline requires 48kHz, add a `resampler` node after the TTS node.

### Model Attribution

- **Kokoro models**: Apache 2.0 License
- **Source**: K2-FSA project - https://github.com/k2-fsa/sherpa-onnx
- **Model page**: https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models

## Troubleshooting

### "Model files not found"

Ensure you've downloaded models:
```bash
just download-kokoro-models
```

Check that `model_dir` parameter points to the correct location.

### "TTS generation failed"

- Check that models are complete (re-download if corrupted)
- Verify sufficient RAM (~1 GB available)
- Try reducing `num_threads` if CPU is overloaded

### Slow performance (RTF > 1.0)

- Increase `num_threads` (try 4-8)
- Check CPU usage (should be near 100% during generation)
- Ensure no other heavy processes running
- If using a GPU-enabled build, set `execution_provider: "cuda"` (or `"tensorrt"` if available)

### Audio quality issues

- Try different `speaker_id` values (voices vary in quality)
- Adjust `speed` parameter (too fast/slow can affect quality)
- Check sentence boundary detection isn't cutting off mid-word

## Development

### Running Tests

```bash
cd plugins/native
cargo test
```

### Debugging

Enable logging in plugin:
```rust
eprintln!("TTS Debug: generating for text: {}", text);
```

Check server logs for plugin loading issues.

## License

- **Code**: MPL-2.0 (StreamKit Contributors)
- **Kokoro Models**: Apache 2.0 (K2-FSA/Sherpa-ONNX project)

## Future Enhancements

- [ ] Word-level timestamps (requires model update)
- [ ] Voice cloning (requires different model)
- [ ] Multi-instance pooling for parallelism
- [ ] Streaming with callbacks (lower latency)
