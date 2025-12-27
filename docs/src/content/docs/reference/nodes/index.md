---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Node Reference
description: Built-in node kinds and parameter reference
---

This section documents the **built-in nodes** shipped with StreamKit, including their pins and parameter schema.

Available nodes (including loaded plugins) are also discoverable at runtime:

```bash
curl http://localhost:4545/api/v1/schema/nodes
```

Notes:

- The response is permission-filtered based on your role.
- Two synthetic nodes exist for oneshot-only HTTP streaming: `streamkit::http_input` and `streamkit::http_output`.


## `audio` (8)

- [`audio::flac::decoder`](./audio-flac-decoder/)
- [`audio::gain`](./audio-gain/)
- [`audio::mixer`](./audio-mixer/)
- [`audio::mp3::decoder`](./audio-mp3-decoder/)
- [`audio::opus::decoder`](./audio-opus-decoder/)
- [`audio::opus::encoder`](./audio-opus-encoder/)
- [`audio::pacer`](./audio-pacer/)
- [`audio::resampler`](./audio-resampler/)

## `containers` (4)

- [`containers::ogg::demuxer`](./containers-ogg-demuxer/)
- [`containers::ogg::muxer`](./containers-ogg-muxer/)
- [`containers::wav::demuxer`](./containers-wav-demuxer/)
- [`containers::webm::muxer`](./containers-webm-muxer/)

## `core` (10)

- [`core::file_reader`](./core-file-reader/)
- [`core::file_writer`](./core-file-writer/)
- [`core::json_serialize`](./core-json-serialize/)
- [`core::pacer`](./core-pacer/)
- [`core::passthrough`](./core-passthrough/)
- [`core::script`](./core-script/)
- [`core::sink`](./core-sink/)
- [`core::telemetry_out`](./core-telemetry-out/)
- [`core::telemetry_tap`](./core-telemetry-tap/)
- [`core::text_chunker`](./core-text-chunker/)

## `streamkit` (2)

- [`streamkit::http_input`](./streamkit-http-input/)
- [`streamkit::http_output`](./streamkit-http-output/)

## `transport` (4)

- [`transport::http::fetcher`](./transport-http-fetcher/)
- [`transport::moq::peer`](./transport-moq-peer/)
- [`transport::moq::publisher`](./transport-moq-publisher/)
- [`transport::moq::subscriber`](./transport-moq-subscriber/)
