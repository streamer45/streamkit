---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::webm::muxer"
description: "Muxes Opus audio and/or VP9/AV1 video into a WebM container. Produces streamable WebM output compatible with web browsers. Supports audio-only, video-only, or combined audio+video muxing."
---

`kind`: `containers::webm::muxer`

Muxes Opus audio and/or VP9/AV1 video into a WebM container. Produces streamable WebM output compatible with web browsers. Supports audio-only, video-only, or combined audio+video muxing.

## Categories
- `containers`
- `webm`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint32)` | no | `2` | Number of audio channels (1 for mono, 2 for stereo)<br />min: `0` |
| `num_inputs` | `integer (uint32)` | no | `1` | Number of input pins to declare (1 or 2).<br /><br />Set to 2 for pipelines that feed both audio and video into the muxer<br />(e.g. `needs: { in: opus_encoder, in_1: vp9_encoder }`).  Defaults<br />to 1 for single-input (audio-only or video-only) pipelines.<br />min: `1`<br />max: `2` |
| `opus_preskip_samples` | `integer (uint16)` | no | `312` | Opus encoder lookahead in samples at 48 kHz, written to the OpusHead<br />`pre_skip` field.  Decoders use this to trim encoder delay.<br />Default: 312 (typical libopus default).<br />min: `0`<br />max: `65535` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz (used when an audio input is connected)<br />min: `0` |
| `streaming_mode` | `string` | no | — | — |
| `video_height` | `integer (uint32)` | no | `0` | Video height in pixels (required when a video input is connected)<br />min: `0` |
| `video_width` | `integer (uint32)` | no | `0` | Video width in pixels (required when a video input is connected)<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "WebMStreamingMode": {
      "oneOf": [
        {
          "const": "live",
          "description": "Live streaming mode - optimized for real-time streaming, no duration/seeking info (default)",
          "type": "string"
        },
        {
          "const": "file",
          "description": "File mode - includes full duration and seeking information",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "channels": {
      "default": 2,
      "description": "Number of audio channels (1 for mono, 2 for stereo)",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "num_inputs": {
      "default": 1,
      "description": "Number of input pins to declare (1 or 2).\n\nSet to 2 for pipelines that feed both audio and video into the muxer\n(e.g. `needs: { in: opus_encoder, in_1: vp9_encoder }`).  Defaults\nto 1 for single-input (audio-only or video-only) pipelines.",
      "format": "uint32",
      "maximum": 2,
      "minimum": 1,
      "type": "integer"
    },
    "opus_preskip_samples": {
      "default": 312,
      "description": "Opus encoder lookahead in samples at 48 kHz, written to the OpusHead\n`pre_skip` field.  Decoders use this to trim encoder delay.\nDefault: 312 (typical libopus default).",
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": "integer"
    },
    "sample_rate": {
      "default": 48000,
      "description": "Audio sample rate in Hz (used when an audio input is connected)",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "streaming_mode": {
      "$ref": "#/$defs/WebMStreamingMode",
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files\nwith duration (default)"
    },
    "video_height": {
      "default": 0,
      "description": "Video height in pixels (required when a video input is connected)",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "video_width": {
      "default": 0,
      "description": "Video width in pixels (required when a video input is connected)",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "WebMMuxerConfig",
  "type": "object"
}
```

</details>
