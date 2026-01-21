<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Speech Gateway

Thin HTTP gateway that rewrites simple STT/TTS requests into the multipart oneshot format expected by a StreamKit backend.

## Prereqs

- StreamKit server running locally (default assumed: `http://127.0.0.1:4545`).
- Go 1.24+.

## Run the gateway

```sh
cd examples/streamkit-cli-gateway
go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545
```

Environment equivalents:

- `GATEWAY_LISTEN` (default `:8080`)
- `SKIT_URL` (default `http://127.0.0.1:4545`)
- `SKIT_TOKEN` (optional bearer sent to Skit)
- `GATEWAY_MAX_CONCURRENCY` (default 10)
- `GATEWAY_MAX_BODY_BYTES` (default 10MB)

## STT via curl (Ogg/Opus)

Transcribe a file:

```sh

curl -H "Content-Type: audio/ogg" --data-binary @speech.ogg http://127.0.0.1:8080/stt
```

Response is NDJSON (one JSON object per line).

## TTS via curl (plain text)

```sh
curl -H "Content-Type: text/plain" --data 'Hello from StreamKit' http://127.0.0.1:8080/tts | ffplay -nodisp -autoexit -
```

Response is `audio/ogg` (Opus mono).
