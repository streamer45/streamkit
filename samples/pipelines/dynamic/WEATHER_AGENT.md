<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Real-Time Voice Weather (OpenAI + Open-Meteo)

This sample pipeline turns voice questions into spoken weather answers:

- MoQ audio input → Whisper STT
- `core::script` uses OpenAI to parse the request, then calls Open-Meteo (geocoding + forecast)
- Kokoro TTS → MoQ audio output

## Pipeline

- `samples/pipelines/dynamic/voice-weather-open-meteo.yaml`
- Weather script: `samples/pipelines/dynamic/voice-weather-open-meteo.js`

## Prerequisites

- Whisper STT model: `models/ggml-base.en-q5_1.bin`
- Silero VAD model: `models/silero_vad.onnx`
- Kokoro model dir: `models/kokoro-multi-lang-v1_1`

## Server Configuration

`fetch()` from `core::script` is blocked by default. Add these entries to your `skit.toml`:

```toml
[[script.global_fetch_allowlist]]
url = "https://api.openai.com/v1/chat/completions"
methods = ["POST"]

[[script.global_fetch_allowlist]]
url = "https://geocoding-api.open-meteo.com/*"
methods = ["GET"]

[[script.global_fetch_allowlist]]
url = "https://api.open-meteo.com/*"
methods = ["GET"]

[script.secrets.openai_key]
env = "OPENAI_API_KEY"
type = "apikey"
allowed_fetch_urls = ["https://api.openai.com/*"]
description = "OpenAI API key for the voice weather sample pipeline"
```

Set the environment variable before starting `skit`:

```bash
export OPENAI_API_KEY="sk-..."
```

## Prompts to Try

- “What’s the weather in Berlin today?”
- “Will it rain in Seattle tomorrow?”
- “What’s the weather now in Tokyo, in Fahrenheit?”
