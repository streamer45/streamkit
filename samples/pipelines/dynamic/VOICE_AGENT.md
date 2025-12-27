<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Real-Time Voice Agent (OpenAI)

This document describes a real-time voice-to-voice agent pipeline built with StreamKit.

## Overview

The voice agent pipeline provides a complete voice-to-voice conversation system:

1. **Audio Input** - Receive audio from MoQ subscriber (client speaking)
2. **Speech-to-Text** - Transcribe speech using Whisper STT with VAD
3. **LLM Processing** - Send transcription to OpenAI Chat Completions with a system prompt
4. **Text-to-Speech** - Convert AI response to speech using Kokoro TTS
5. **Audio Output** - Stream audio back via MoQ publisher

The Stream View telemetry timeline shows:
- VAD speech start/end events (from Whisper)
- STT transcription previews
- LLM request/response spans (with latency)
- TTS start/done events (with latency)

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Voice Agent Pipeline                     │
└─────────────────────────────────────────────────────────────────┘

[MoQ Subscriber]           Opus Audio from client
      ↓
[Opus Decoder]             Decode to PCM (48kHz)
      ↓
[Audio Resampler]          Convert to 16kHz mono (Whisper requirement)
      ↓
[Whisper STT]              Audio → Text (with VAD segmentation)
      ↓
[Script: OpenAI]           Text → LLM Response (with secrets)
      ↓
[Kokoro TTS]               Text → Audio (24kHz mono)
      ↓
[Audio Resampler]          Upsample to 48kHz mono
      ↓
[Opus Encoder]             PCM → Opus
      ↓
[MoQ Publisher]            Stream back to client
```

## Prerequisites

### 1. Models

Download and place the following models in the `models/` directory:

- **Whisper STT**: `ggml-base.en-q5_1.bin` (~140 MB)
- **VAD**: `silero_vad.onnx` (~3.5 MB)
- **Kokoro TTS**: `kokoro-multi-lang-v1_1/` directory (~360 MB)

```bash
# Download Kokoro models
just download-kokoro-models

# Whisper models are typically downloaded separately
# See: https://github.com/ggerganov/whisper.cpp/tree/master/models
```

### 2. Server Configuration

Add the following to your `skit.toml`:

```toml
# Global fetch allowlist for OpenAI API
[[script.global_fetch_allowlist]]
url = "https://api.openai.com/v1/chat/completions"
methods = ["POST"]

# OpenAI API key secret
[script.secrets.openai_key]
env = "OPENAI_API_KEY"
type = "apikey"
allowed_fetch_urls = ["https://api.openai.com/*"]
description = "OpenAI API key for the voice agent sample pipeline"
```

### 3. Environment Variables

Set your OpenAI API key:

```bash
export OPENAI_API_KEY="sk-proj-..."
```

### 4. MoQ Server

Ensure you have a MoQ server running on the configured port (default: 4443).

## Usage

### Starting the Server

```bash
# Start skit server with the configuration
just skit serve

# Or with debug logging to see pipeline activity
RUST_LOG=debug just skit serve
```

### Creating a Session

Use the API or UI to create a session with this pipeline:

```bash
python3 - <<'PY' | curl -X POST http://localhost:4545/api/v1/sessions \
  -H "Content-Type: application/json" \
  -d @-
import json
from pathlib import Path

pipeline_yaml = Path("samples/pipelines/dynamic/voice-agent-openai.yaml").read_text()
print(json.dumps({"name": "voice-agent", "yaml": pipeline_yaml}))
PY
```

### Connecting a Client

Connect your MoQ client to:
- **Publish audio to**: `input` broadcast
- **Subscribe to audio from**: `output` broadcast

## Configuration

### Whisper STT Tuning

Adjust speech detection and segmentation:

```yaml
whisper_stt:
  params:
    vad_threshold: 0.5              # 0.0-1.0 (higher = less sensitive)
    min_silence_duration_ms: 700    # Wait before ending segment
    max_segment_duration_secs: 30.0 # Force segment after duration
    n_threads: 0                    # 0 = auto, or specify 1-16
```

### OpenAI LLM Tuning

Modify the script node to adjust LLM behavior:

```javascript
{
  model: 'gpt-4-turbo',       // or 'gpt-3.5-turbo' for speed
  max_tokens: 200,            // Shorter = faster responses
  temperature: 0.7,           // 0.0-2.0 (higher = more creative)
}
```

### Kokoro TTS Tuning

Adjust voice and speech characteristics:

```yaml
kokoro_tts:
  params:
    speaker_id: 0             # 0-102 (different voices)
    speed: 1.0                # 0.5-2.0 (speech rate)
    min_sentence_length: 10   # Buffer size before TTS
```

### System Prompt Customization

Edit the script node to change the AI assistant's behavior:

```javascript
{
  role: 'system',
  content: 'You are a helpful voice assistant. Keep responses brief and conversational, suitable for speech.'
}
```

## Performance

### Expected Latencies

| Component | Latency | Notes |
|-----------|---------|-------|
| MoQ Transport | 20-100ms | Network + WebTransport overhead |
| Opus Decode | <5ms | Near-instant |
| Whisper STT | 700ms-5s | VAD-based, depends on speech patterns |
| OpenAI API | 1-5s | Network + LLM processing |
| Kokoro TTS | 500ms-1s | Per sentence, streaming |
| Opus Encode | <5ms | Near-instant |
| **Total E2E** | **2-12s** | Typical: 3-6s for short responses |

### Resource Usage

- **CPU**: 20-40% during speech processing (4+ cores)
- **Memory**: ~1.5 GB (models + runtime)
- **Network**: ~128 kbps per direction (Opus)

## Troubleshooting

### Issue: "Fetch blocked: Global allowlist is empty"

**Solution**: Add OpenAI URL to `script.global_fetch_allowlist` in `skit.toml`

### Issue: "Secret 'openai_key' not found"

**Solution**:
1. Add `[script.secrets.openai_key]` to `skit.toml`
2. Set `export OPENAI_API_KEY="..."`
3. Restart server

### Issue: Whisper validation error "Expected 16kHz mono"

**Solution**: Ensure resampler is configured with:
```yaml
target_sample_rate: 16000
channels: 1
```

### Issue: OpenAI API errors

**Solution**:
- Check API key validity
- Verify internet connectivity
- Check `timeout_ms` is sufficient (15000+ recommended)
- Monitor rate limits

### Issue: No audio output

**Solution**:
- Verify MoQ server is running on port 4443
- Check broadcast names match ("input"/"output")
- Ensure Opus encoder bitrate is set
- Verify resampler outputs 48kHz

## Security Model

```
Server Config (skit.toml)          Pipeline YAML                JavaScript
─────────────────────────          ─────────────                ──────────
[script.secrets]                   headers:                     fetch(url, {
  openai_key:                        - secret: openai_key         method: 'POST',
    env: OPENAI_API_KEY               header: Authorization        body: {...}
    type: apikey                      template: "Bearer {}"      })

Server validates:                  Node validates:              Runtime validates:
✓ Secret exists in env             ✓ Secret in allowlist        ✓ URL in allowlist
✓ Secret declared                  ✓ Header mapping valid       ✓ Method allowed
```

**Key Principle**: API key is NEVER exposed to JavaScript - injected by Rust at HTTP layer.

## Advanced Customization

### Using Different Models

**GPT-3.5 Turbo** (faster, cheaper):
```javascript
model: 'gpt-3.5-turbo'
```

**Different Whisper Model**:
```yaml
model_path: "models/ggml-large-v3.bin"  # Better accuracy, slower
```

**Different Voice**:
```yaml
speaker_id: 10  # Try different values 0-102 for English
```

### Adding Conversation History

The current implementation is stateless. To add conversation history:

1. Store previous messages in a database or Redis
2. Modify the script to fetch conversation history
3. Include previous messages in the OpenAI API call

### Streaming Responses

For lower perceived latency, consider:

1. Using OpenAI's streaming API (SSE)
2. Sending partial responses to TTS as they arrive
3. Adjusting `min_sentence_length` in Kokoro TTS

## Examples

See the complete pipeline definition in:
- `samples/pipelines/dynamic/voice-agent-openai.yaml`

Related examples:
- `samples/pipelines/dynamic/moq.yml` - Basic MoQ streaming
- `samples/pipelines/kokoro-tts.yml` - Kokoro TTS usage
- `samples/pipelines/script-webhook-demo.yaml` - Script node with secrets

## License

This pipeline configuration is provided under the MPL-2.0 license.
