---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::mp4::muxer"
description: "Muxes H.264/AV1 video and/or AAC/Opus audio into an MP4 container. Supports fragmented MP4 (fMP4) for DASH/HLS streaming and regular MP4 file output with fast-start."
---

`kind`: `containers::mp4::muxer`

Muxes H.264/AV1 video and/or AAC/Opus audio into an MP4 container. Supports fragmented MP4 (fMP4) for DASH/HLS streaming and regular MP4 file output with fast-start.

## Categories
- `containers`
- `mp4`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None }), Binary` (one)

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `audio_codec` | `null | string enum[opus, aac]` | no | `null` | Override the audio codec used for sample-entry construction and MIME<br />content-type resolution.  When omitted the codec is auto-detected from<br />the upstream `EncodedAudio` pin type; if detection fails it falls back<br />to `Opus`. |
| `audio_timescale` | `integer (uint32)` | no | `48000` | Audio timescale (ticks per second).  Default: 48000.<br />min: `0` |
| `channels` | `integer (uint16)` | no | `2` | Number of audio channels (1 = mono, 2 = stereo).<br />min: `0`<br />max: `65535` |
| `mode` | `string` | no | — | MP4 muxer streaming mode. |
| `num_inputs` | `integer (uint32)` | no | `1` | Number of input pins (1 or 2).<br />min: `1`<br />max: `2` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz.<br />min: `0` |
| `video_codec` | `null | value` | no | `null` | Override the video codec used for the pre-connection MIME content-type<br />hint.  When omitted, the hint defaults to AV1 (if video dimensions<br />are set).  The runtime MIME type is always resolved from the actual<br />input codec. |
| `video_height` | `integer (uint16)` | no | `0` | Video height in pixels (used for sample entry construction).<br />min: `0`<br />max: `65535` |
| `video_timescale` | `integer (uint32)` | no | `90000` | Video timescale (ticks per second).  Default: 90000.<br />min: `0` |
| `video_width` | `integer (uint16)` | no | `0` | Video width in pixels (used for sample entry construction).<br />min: `0`<br />max: `65535` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "AudioCodec": {
      "description": "Supported encoded audio codecs.",
      "enum": [
        "opus",
        "aac"
      ],
      "type": "string"
    },
    "Mp4StreamingMode": {
      "description": "MP4 muxer streaming mode.",
      "oneOf": [
        {
          "const": "stream",
          "description": "Fragmented MP4 (fMP4) mode — produces segments suitable for DASH/HLS\nstreaming.  Each segment is sent downstream immediately.",
          "type": "string"
        },
        {
          "const": "file",
          "description": "Regular MP4 file mode — writes to a temp file and sends the complete\nfile after finalization.  Supports fast-start (moov before mdat).",
          "type": "string"
        }
      ]
    },
    "VideoCodec": {
      "description": "Supported encoded video codecs.",
      "oneOf": [
        {
          "enum": [
            "vp9"
          ],
          "type": "string"
        },
        {
          "const": "h264",
          "description": "OpenH264 Constrained Baseline encoder/decoder.",
          "type": "string"
        },
        {
          "const": "av1",
          "description": "CPU AV1 codec support via rav1e (encoder) and rav1d (decoder).",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the MP4 muxer node.",
  "properties": {
    "audio_codec": {
      "anyOf": [
        {
          "$ref": "#/$defs/AudioCodec"
        },
        {
          "type": "null"
        }
      ],
      "default": null,
      "description": "Override the audio codec used for sample-entry construction and MIME\ncontent-type resolution.  When omitted the codec is auto-detected from\nthe upstream `EncodedAudio` pin type; if detection fails it falls back\nto `Opus`."
    },
    "audio_timescale": {
      "default": 48000,
      "description": "Audio timescale (ticks per second).  Default: 48000.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "channels": {
      "default": 2,
      "description": "Number of audio channels (1 = mono, 2 = stereo).",
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": "integer"
    },
    "mode": {
      "$ref": "#/$defs/Mp4StreamingMode",
      "description": "Streaming mode: `\"stream\"` for fMP4 segments, `\"file\"` for regular MP4."
    },
    "num_inputs": {
      "default": 1,
      "description": "Number of input pins (1 or 2).",
      "format": "uint32",
      "maximum": 2,
      "minimum": 1,
      "type": "integer"
    },
    "sample_rate": {
      "default": 48000,
      "description": "Audio sample rate in Hz.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "video_codec": {
      "anyOf": [
        {
          "$ref": "#/$defs/VideoCodec"
        },
        {
          "type": "null"
        }
      ],
      "default": null,
      "description": "Override the video codec used for the pre-connection MIME content-type\nhint.  When omitted, the hint defaults to AV1 (if video dimensions\nare set).  The runtime MIME type is always resolved from the actual\ninput codec."
    },
    "video_height": {
      "default": 0,
      "description": "Video height in pixels (used for sample entry construction).",
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": "integer"
    },
    "video_timescale": {
      "default": 90000,
      "description": "Video timescale (ticks per second).  Default: 90000.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "video_width": {
      "default": 0,
      "description": "Video width in pixels (used for sample entry construction).",
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "Mp4MuxerConfig",
  "type": "object"
}
```

</details>
