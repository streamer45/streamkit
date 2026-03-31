<!--
SPDX-FileCopyrightText: Copyright (c) 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Pocket TTS Native Plugin

A native StreamKit plugin for Kyutai Pocket TTS using the Rust/Candle port.
Upstream Rust port: https://github.com/babybirdprd/pocket-tts
This plugin runs fully on CPU and streams 24kHz mono audio.

## Build

```bash
just build-plugin-native-pocket-tts
```

Plugin binary:
`target/plugins/release/libpocket_tts.so`

## Download models (offline-friendly)

This prefetches weights, tokenizer, and stock voice embeddings into the HF cache
(`models/hf`) and mirrors voice embeddings into `models/pocket-tts/embeddings`.
The main weights repo is gated, so set `HF_TOKEN` first.

```bash
HF_TOKEN=your_token_here just download-pocket-tts-models
```

To use the cached files offline, keep `HF_HOME` consistent when running the server
and set `HF_HUB_OFFLINE=1`:

```bash
export HF_HOME=models/hf
export HF_HUB_OFFLINE=1
```

If you want fully local paths (no HF cache at runtime), pass `weights_path` and
`tokenizer_path`, and optionally set `voice_embeddings_dir`:

```yaml
params:
  weights_path: "models/pocket-tts/tts_b6369a24.safetensors"
  tokenizer_path: "models/pocket-tts/tokenizer.model"
  voice_embeddings_dir: "models/pocket-tts/embeddings"
  voice: alba
```

## Loading the plugin

```bash
curl -X POST -F plugin=@target/plugins/release/libpocket_tts.so \
  http://127.0.0.1:4545/api/v1/plugins
```

The plugin appears as: `plugin::native::pocket-tts`.

## Usage

```yaml
nodes:
  tts:
    kind: plugin::native::pocket-tts
    params:
      voice: alba
      temperature: 0.7
      lsd_decode_steps: 1
      eos_threshold: -4.0
```

## Parameters

- `variant` (string): Model variant config (default: `b6369a24`).
- `config_path` (string | null): Optional config YAML path for custom variants/offline use.
- `weights_path` (string | null): Local weights path for offline loading.
- `tokenizer_path` (string | null): Local tokenizer path for offline loading.
- `voice_embeddings_dir` (string | null): Directory containing predefined voice embeddings.
- `voice` (string): Voice name, local `.wav`/`.safetensors`, `hf://` URL, or base64 audio (default: `alba`).
- `temperature` (number): Sampling temperature (default: `0.7`).
- `lsd_decode_steps` (int): LSD decode steps (default: `1`).
- `eos_threshold` (number): End-of-sequence threshold (default: `-4.0`).
- `noise_clamp` (number | null): Optional noise clamp (default: `null`).
- `min_sentence_length` (int): Minimum characters before TTS triggers (default: `10`).
- `quantized` (bool): Enable int8 weights (requires plugin built with feature `quantized`).

## Voices

Predefined voices:
`alba`, `marius`, `javert`, `jean`, `fantine`, `cosette`, `eponine`, `azelma`

You can also pass:
- Local `.wav` (voice cloning)
- Local `.safetensors` (precomputed embeddings)
- `hf://` URLs
- Base64-encoded WAV (with or without `data:audio/...;base64,` prefix)

## Notes

- Weights and tokenizer are downloaded via Hugging Face on first use unless you
  provide `weights_path` and `tokenizer_path`.
- Model licenses and voice usage terms are governed by Kyutai; review upstream usage restrictions.

## License

- Plugin code: MPL-2.0 (StreamKit Contributors)
- Pocket TTS models/voices: see upstream licenses and terms.
