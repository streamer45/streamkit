---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::webm::muxer"
description: "Muxes Opus audio and/or VP9 video into a WebM container. Produces streamable WebM output compatible with web browsers. Supports audio-only, video-only, or combined audio+video muxing."
---

`kind`: `containers::webm::muxer`

Muxes Opus audio and/or VP9 video into a WebM container. Produces streamable WebM output compatible with web browsers. Supports audio-only, video-only, or combined audio+video muxing.

## Categories
- `containers`
- `webm`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

> **Dual-pin muxing:** When `video_width` and `video_height` are set to non-zero values, a second input pin (`in_1`) is created automatically to accept the video stream. Connect your audio encoder to `in` and your video encoder to `in_1` for combined audio+video WebM output.

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint32)` | no | `2` | Number of audio channels (1 for mono, 2 for stereo)<br />min: `0` |
| `chunk_size` | `integer (uint)` | no | `65536` | The number of bytes to buffer before flushing to the output. Defaults to 65536.<br />min: `0` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz (used when an audio input is connected)<br />min: `0` |
| `streaming_mode` | `string` | no | — | — |
| `video_height` | `integer (uint32)` | no | `0` | Video height in pixels (required when a video input is connected)<br />min: `0` |
| `video_width` | `integer (uint32)` | no | `0` | Video width in pixels (required when a video input is connected)<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "WebMMuxerConfig",
  "type": "object",
  "properties": {
    "sample_rate": {
      "description": "Audio sample rate in Hz (used when an audio input is connected)",
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
    "video_width": {
      "description": "Video width in pixels (required when a video input is connected)",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 0
    },
    "video_height": {
      "description": "Video height in pixels (required when a video input is connected)",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 0
    },
    "chunk_size": {
      "description": "The number of bytes to buffer before flushing to the output. Defaults to 65536.",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 65536
    },
    "streaming_mode": {
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files\nwith duration (default)",
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
