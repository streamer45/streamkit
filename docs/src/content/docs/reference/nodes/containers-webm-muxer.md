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

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint32)` | no | `2` | Number of audio channels (1 for mono, 2 for stereo)<br />min: `0` |
| `opus_preskip_samples` | `integer (uint16)` | no | `312` | Opus encoder lookahead in samples at 48 kHz, written to the OpusHead<br />`pre_skip` field.  Decoders use this to trim encoder delay.<br />Default: 312 (typical libopus default).<br />min: `0`<br />max: `65535` |
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
    "streaming_mode": {
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files\nwith duration (default)",
      "$ref": "#/$defs/WebMStreamingMode"
    },
    "opus_preskip_samples": {
      "description": "Opus encoder lookahead in samples at 48 kHz, written to the OpusHead\n`pre_skip` field.  Decoders use this to trim encoder delay.\nDefault: 312 (typical libopus default).",
      "type": "integer",
      "format": "uint16",
      "minimum": 0,
      "maximum": 65535,
      "default": 312
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
