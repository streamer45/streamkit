# Supertonic TTS Plugin

Multilingual text-to-speech plugin for StreamKit using the [Supertonic](https://github.com/supertone-inc/supertonic) TTS engine.

## Features

- 66M parameter model, up to 167x faster than real-time
- 5 languages: English, Korean, Spanish, Portuguese, French
- 10 voice styles: M1-M5 (male), F1-F5 (female)
- ONNX Runtime-based inference (4 models: duration predictor, text encoder, vector estimator, vocoder)
- Global model caching across pipeline nodes

## Setup

```bash
# Download models
just download-supertonic-models

# Build plugin
just build-plugin-native-supertonic
```

## Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `model_dir` | string | `./models/supertonic-v2-onnx` | Path to ONNX model directory |
| `lang` | string | `en` | Language: `en`, `ko`, `es`, `pt`, `fr` |
| `voice_style` | string | `M1` | Style name (M1-M5, F1-F5) or `.json` path |
| `voice_styles_dir` | string | - | Directory for named voice style files |
| `total_step` | integer | `5` | Denoising steps (1-20, higher = better quality) |
| `speed` | number | `1.05` | Speech speed multiplier (0.5-2.0) |
| `silence_duration` | number | `0.3` | Silence between chunks in seconds |
| `min_sentence_length` | integer | `10` | Minimum chars before triggering TTS |
| `emit_telemetry` | boolean | `false` | Emit tts.start/tts.done telemetry events |

## License

Plugin code: MPL-2.0
Supertonic engine: MIT
