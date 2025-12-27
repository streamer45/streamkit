<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Piper TTS Native Plugin

Fast, low-latency streaming text-to-speech using Piper VITS models via Sherpa-ONNX.

## Features

- 🚀 **Fast & lightweight** - Optimized for low latency and CPU efficiency
- 🎙️ **20+ English voices** - Multiple high-quality voices to choose from
- ⚡ **Real-time on CPU** - Faster than Kokoro, runs well on modest hardware
- 🔄 **Streaming output** - Sentence-level streaming for LLM integration
- 🎵 **16-22 kHz audio** - Good quality with smaller model sizes
- 🦀 **Safe Rust implementation** (sherpa-rs)

## Setup

### 1. Download Models (one-time, ~80 MB)

```bash
cd plugins/native
./download-models.sh
```

This downloads the `vits-piper-en_US-lessac-medium` model (pre-converted for sherpa-onnx) which includes:
- model.onnx (63 MB) - VITS TTS model
- model.onnx.json - Model configuration
- tokens.txt - Phoneme vocabulary
- espeak-ng-data/ - Phoneme conversion data (shared across models)

### 2. Build Plugin

```bash
just build-plugin-native-piper
```

### 3. Upload to Server

```bash
just upload-piper-plugin
```

Or manually:
```bash
curl -X POST \
  -F plugin=@target/release/libpiper.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### 4. Verify Loaded

```bash
curl http://localhost:4545/api/v1/plugins
# Should show: plugin::native::piper
```

## Usage

### Example Pipelines

See `samples/pipelines/oneshot/piper-tts.yml` for a complete oneshot example.

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | *required* | Path to Piper model directory |
| `speaker_id` | integer | 0 | Voice selection (for multi-speaker models) |
| `speed` | number | 1.0 | Speech rate (0.5-2.0) |
| `num_threads` | integer | 4 | CPU threads for inference (1-16) |
| `min_sentence_length` | integer | 10 | Chars to buffer before TTS |
| `noise_scale` | number | 0.667 | Controls voice variation (0.0-1.0) |
| `noise_scale_w` | number | 0.8 | Controls prosody variation (0.0-1.0) |
| `length_scale` | number | 1.0 | Affects speech duration (0.5-2.0) |

### Example Configuration

```yaml
steps:
  - kind: plugin::native::piper
    params:
      model_dir: "models/vits-piper-en_US-libritts_r-medium"
      speed: 1.2      # Speak a bit faster
      noise_scale: 0.7   # More natural variation
```

## Performance

### Benchmarks (vs Kokoro)

| Metric | Piper (medium) | Kokoro (v1.1) |
|--------|----------------|---------------|
| **Model size** | 63 MB | 310 MB |
| **Memory usage** | ~200 MB | ~600 MB |
| **Latency** | 200-500ms/sentence | 500-1000ms/sentence |
| **Quality** | Good (22kHz) | High (24kHz) |
| **CPU usage** | Lower | Higher |
| **Real-time factor** | ~0.3-0.7 | ~0.5-1.5 |

*Lower RTF = faster generation*

### Expected Performance

| CPU | Cores | Piper RTF | Real-time? |
|-----|-------|-----------|------------|
| Intel i7 (modern) | 4 | ~0.4 | ✅ Yes |
| AMD Ryzen 5 | 4 | ~0.3 | ✅ Yes |
| Apple M1 | 4 | ~0.2 | ✅ Yes |
| Raspberry Pi 4 | 4 | ~1.5 | ✅ Yes (marginal) |

**Piper is significantly faster than Kokoro** due to smaller model size and optimized VITS architecture.

## Architecture

### Streaming Model

The plugin uses **sentence-based streaming** (same as Kokoro):

1. **Text arrives** from upstream (e.g., LLM or text_chunker)
2. **Buffer accumulates** text until sentence boundary (`. ! ?`)
3. **TTS generates** audio for complete sentence
4. **Audio emitted** immediately (no waiting for full text)
5. **Process repeats** for next sentence

This balances latency vs. quality.

### Data Flow

```
Text Packets → Text Buffer → Sentence Splitter → TTS Engine → Audio Packets (22kHz)
```

## Technical Details

### Dependencies

- **sherpa-rs** (0.6+): Rust bindings to sherpa-onnx
- **streamkit-plugin-sdk-native**: StreamKit plugin SDK
- **unicode-segmentation**: Sentence boundary detection

### Audio Output

- **Sample rate**: 22050 Hz (typical for Piper models, varies by model)
- **Channels**: Mono (1 channel)
- **Format**: F32 samples in range [-1.0, 1.0]
- **Frame size**: Variable (depends on sentence length)

**Note**: Check `model.onnx.json` for exact sample rate of your chosen model.

### Available Models

Piper has 20+ English voices available at https://huggingface.co/rhasspy/piper-voices

Popular models:
- **lessac** (medium) - Natural, clear male voice
- **amy** (low/medium) - Clear female voice
- **libritts** (medium) - Natural, diverse (904 speakers!)
- **ljspeech** (high) - Female, higher quality

Download other models by modifying `download-models.sh` or manually fetching from Hugging Face.

## Comparison: Piper vs Kokoro

### When to Use Piper

✅ **Use Piper when:**
- You need **low latency** and fast generation
- Running on **resource-constrained** hardware
- You want **smaller memory footprint**
- Good quality is sufficient (not highest fidelity needed)
- You need multiple **voice options** easily

### When to Use Kokoro

✅ **Use Kokoro when:**
- You need **highest audio quality**
- You have sufficient CPU/memory resources
- You want **multilingual** support (Chinese + English)
- You need **103 different voices**

## Troubleshooting

### "Model files not found"

Ensure you've downloaded models:
```bash
cd plugins/native
./download-models.sh
```

Check that `model_dir` parameter points to the correct location.

### "TTS generation failed"

- Check that models are complete (re-run download script)
- Verify sufficient RAM (~500 MB available)
- Try reducing `num_threads` if CPU is overloaded
- Check logs for espeak-ng errors (phonemization issues)

### Slow performance (RTF > 1.0)

- Increase `num_threads` (try 4-8)
- Try a lower-quality model (e.g., "low" instead of "medium")
- Check CPU usage during generation
- Ensure no other heavy processes running

### Audio quality issues

- Try different `noise_scale` values (0.5-0.8 range)
- Adjust `noise_scale_w` for prosody changes
- Try a different model (some voices vary in quality)
- Check `length_scale` isn't too extreme

## Development

### Running Tests

```bash
cd plugins/native
cargo test
```

### Debugging

Enable logging:
```rust
tracing::info!("TTS Debug: generating for text: {}", text);
```

Check server logs for plugin loading issues.

## Future Enhancements

- [ ] **Callback-based streaming** (chunk-level, even lower latency)
- [ ] GPU acceleration (CUDA/Metal support)
- [ ] Multi-instance pooling for parallelism
- [ ] Streaming with phoneme-level callbacks
- [ ] Voice cloning support (requires different models)

## License

- **Code**: MPL-2.0 (StreamKit Contributors)
- **Piper Models**: Various licenses (see model cards on Hugging Face)
- **espeak-ng**: GPL-3.0

## Model Attribution

- **Piper models**: rhasspy/piper-voices (Hugging Face)
- **Sherpa-ONNX**: K2-FSA project - https://github.com/k2-fsa/sherpa-onnx
- **espeak-ng**: https://github.com/espeak-ng/espeak-ng
