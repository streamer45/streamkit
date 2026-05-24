<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Testing the Convert and Stream Views

The Convert view (`/convert`) handles oneshot pipelines (file conversion, TTS, transcription) and the Stream view (`/stream`) handles dynamic MoQ pipelines. Both derive their UI controls from the declarative `client` section in pipeline YAML files.

## Setup
1. Start backend: `SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq SK_SERVER__ADDRESS=127.0.0.1:4545 just skit`
2. Start UI: `just ui`
3. Navigate to `http://localhost:3045/convert` or `http://localhost:3045/stream`

## Installing Marketplace Plugins for E2E Testing
To test actual pipeline execution (not just UI rendering), install plugins via the marketplace:

1. Enable marketplace in `skit.toml`:
   ```toml
   [plugins]
   directory = ".plugins"
   marketplace_enabled = true
   allow_native_marketplace = true
   registries = ["https://streamkit.dev/registry/index.json"]
   models_dir = "models"
   trusted_pubkeys = [
       "untrusted comment: minisign public key 81C485A94492F33F\nRWQ/85JEqYXEgX+2kl7Rwd8AcpVjYciSLzvLggzivbGyIrDPjfmcqjYP\n",
   ]

   [plugins.security]
   allow_model_urls = false
   marketplace_scheme_policy = "https_only"
   marketplace_host_policy = "any"
   ```

2. Install plugins via API:
   ```bash
   # Install Kokoro TTS
   curl -X POST http://127.0.0.1:4545/api/v0/marketplace/install \
     -H 'Content-Type: application/json' \
     -d '{"id": "kokoro", "version": "0.1.0"}'

   # Install Whisper STT
   curl -X POST http://127.0.0.1:4545/api/v0/marketplace/install \
     -H 'Content-Type: application/json' \
     -d '{"id": "whisper", "version": "0.1.1"}'
   ```

3. Plugin bundles download to `.plugins/bundles/<name>/<version>/`
4. Models download to `models/` directory (can be ~365MB for Kokoro, ~50MB for Whisper)
5. After installation, restart the backend to load the new plugins

## Convert View Pipeline Types

### Text-to-Speech (client.input.type: text, client.output.type: audio)
- Template: "Text-to-Speech (Kokoro)"
- Shows textarea with character counter, no file upload
- Button text: "Convert to Speech"
- Section title: "Enter Text to Convert to Speech"
- Output: Audio player with playback controls
- Status shows "Streaming complete!" toast and "Completed (XX KB)"

### Speech-to-Text (client.input.type: file_upload, client.output.type: transcription)
- Template: "Speech-to-Text (Whisper)"
- Shows "Upload File" and "Select Existing Asset" buttons
- Asset list filtered by `asset_tags: [speech]` — only shows speech-tagged samples
- Button text: "Transcribe Audio"
- "Choose Output Mode" section is hidden (transcription mode)
- Output: Transcription text with language badge, no audio player

### No-Input (client.input.type: none, client.output.type: video)
- Template: "Video Compositor Demo"
- Input section is completely hidden
- Button text: "Generate"
- Output mode section renumbers to "3" (since input section is skipped)
- Output: Video element with rendered content

### Trigger (client.input.type: trigger)
- Template: "Useless Facts TTS"
- Similar to no-input but semantically represents a triggered action

## Stream View Pipeline Types

### Audio Publish+Watch (e.g. MoQ Peer Transcoder, MoQ Mixing)
- Shows gateway URL (e.g. `/moq`, `/moq/mixing`)
- Shows input broadcast name and output broadcast name
- Audio-only: no video controls

### Video Watch-Only (e.g. Video Color Bars MoQ)
- Shows gateway URL (e.g. `/moq/video`)
- Shows output broadcast name only (no input)
- Video controls visible

## Testing Tips
- Use Playwright with headless Chromium for automated E2E testing
- Pipeline conversions can be fast (<2s for short TTS) — use polling loops instead of fixed timeouts for completion checks
- Wait for "Streaming complete" or "Transcription complete" toast notifications as completion signals
- The Convert button re-enables after completion — this is another reliable completion indicator
- For STT pipelines, use sample speech assets (speech_2m, speech_10m) rather than uploading files
- Screenshot evidence at each step is valuable for verification

## Relevant Files
- `ui/src/views/ConvertView.tsx` — Main convert view component
- `ui/src/views/StreamView.tsx` — Main stream view component
- `samples/pipelines/oneshot/` — Oneshot pipeline YAML templates
- `samples/pipelines/dynamic/` — Dynamic pipeline YAML templates
