<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Speech Gateway

Thin HTTP gateway that rewrites simple STT/TTS requests into the multipart oneshot format expected by a StreamKit backend.

## Hosted instance

A free, best-effort public instance runs at `https://tts.streamkit.dev` and `https://stt.streamkit.dev`, so you can try the endpoints below without running anything:

```sh
curl -d 'Hello from StreamKit' https://tts.streamkit.dev | ffplay -nodisp -autoexit -
curl --data-binary @speech.ogg https://stt.streamkit.dev
```

There is no SLA — it may be slow, rate-limited, or offline at any time, and usage is monitored for abuse. Don't send anything sensitive. Run your own (below) to remove those limits.

## Prereqs

- StreamKit server running locally (default assumed: `http://127.0.0.1:4545`).
- Go 1.24+.

## Run the gateway

```sh
cd examples/speech-gateway
go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545
```

Environment equivalents:

- `GATEWAY_LISTEN` (default `:8080`)
- `SKIT_URL` (default `http://127.0.0.1:4545`)
- `SKIT_TOKEN` (optional bearer sent to Skit)
- `GATEWAY_MAX_CONCURRENCY` (default 10)
- `GATEWAY_MAX_BODY_BYTES` (default 1MB)
- `GATEWAY_MAX_TTS_TEXT_SIZE` (default 1000 characters)

## STT via curl (Ogg/Opus)

Transcribe a file:

```sh
curl -H "Content-Type: audio/ogg" --data-binary @speech.ogg http://127.0.0.1:8080/stt
```

Transcribe from microphone (requires ffmpeg):

```sh
./stt.sh
```

Press Ctrl-C when done speaking. The script captures audio, sends it to the gateway, and displays the transcription.

Response is NDJSON (one JSON object per line).

## TTS via curl (plain text)

```sh
curl -H "Content-Type: text/plain" --data 'Hello from StreamKit' http://127.0.0.1:8080/tts | ffplay -nodisp -autoexit -
```

Response is `audio/ogg` (Opus mono).
