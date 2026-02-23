---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::webm::muxer"
description: "Muxes Opus audio and/or VP9 video into a WebM container. Produces streamable WebM output compatible with web browsers."
---

`kind`: `containers::webm::muxer`

Muxes Opus audio and/or VP9 video into a WebM container. Produces streamable WebM output compatible with web browsers. Supports audio-only, video-only, or combined audio+video muxing.

## Categories
- `containers`
- `webm`

## Pins

Input pins use generic names — the media type (audio or video) is detected at
runtime from each packet's `content_type` field, not from the pin name.

When `video_width` and `video_height` are **not** configured (default), a single
`in` pin is exposed, keeping backward compatibility with existing audio-only
pipelines (`needs: opus_encoder`).

When video dimensions **are** configured, two pins (`in` + `in_1`) are exposed
so that both an audio and a video encoder can be connected. Use the map syntax
to target each pin explicitly:

```yaml
needs:
  in: opus_encoder
  in_1: vp9_encoder
```

### Inputs
- `in` accepts `EncodedAudio(Opus)` or `EncodedVideo(VP9)` (one)
- `in_1` accepts `EncodedAudio(Opus)` or `EncodedVideo(VP9)` (one) — only present when `video_width`/`video_height` > 0

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint32)` | no | `2` | Number of audio channels (1 for mono, 2 for stereo)<br />min: `0` |
| `chunk_size` | `integer (uint)` | no | `65536` | The number of bytes to buffer before flushing to the output. Defaults to 65536.<br />min: `0` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz<br />min: `0` |
| `streaming_mode` | `string` | no | — | Streaming mode: `"live"` for real-time streaming (no duration), `"file"` for complete files with duration |
| `video_width` | `integer (uint32)` | no | `0` | Video frame width in pixels. Set to > 0 together with `video_height` to enable the second input pin for video.<br />min: `0` |
| `video_height` | `integer (uint32)` | no | `0` | Video frame height in pixels. Set to > 0 together with `video_width` to enable the second input pin for video.<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "WebMMuxerConfig",
  "type": "object",
  "properties": {
    "sample_rate": {
      "description": "Audio sample rate in Hz",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 48000
    },
    "channels": {
      "description": "Number of audio channels (1 for mono, 2 for stereo)",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 2
    },
    "chunk_size": {
      "description": "The number of bytes to buffer before flushing to the output. Defaults to 65536.",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 65536
    },
    "streaming_mode": {
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files with duration (default)",
      "$ref": "#/$defs/WebMStreamingMode"
    }
  },
  "$defs": {
    "WebMStreamingMode": {
      "oneOf": [
        {
          "description": "Live streaming mode - optimized for real-time streaming, no duration/seeking info (default)",
          "type": "string",
          "const": "live"
        },
        {
          "description": "File mode - includes full duration and seeking information",
          "type": "string",
          "const": "file"
        }
      ]
    }
  }
}
```

</details>
