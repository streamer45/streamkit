---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::publisher"
description: "Publishes media to a Media over QUIC (MoQ) broadcast. Sends encoded audio and optional video to subscribers over WebTransport."
---

`kind`: `transport::moq::publisher`

Publishes media to a Media over QUIC (MoQ) broadcast. Sends encoded audio and optional video to subscribers over WebTransport.

## Categories
- `transport`
- `moq`
- `dynamic`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)
- `in_1` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `audio` | `boolean | null` | no | `null` | Whether to publish an audio track (Opus on the `in` pin).<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup. In oneshot pipelines this is auto-detected from `input_types`<br />when left as `None`. |
| `audio_codec` | `null | string enum[opus, aac]` | no | `null` | Audio codec for the MoQ catalog.<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup.  When `None`, the codec is auto-detected from `input_types`<br />(static pipelines) and falls back to Opus. |
| `broadcast` | `string` | no | — | — |
| `channels` | `integer (uint32)` | no | `2` | min: `0` |
| `group_duration_ms` | `integer (uint64)` | no | `40` | Duration of each MoQ group in milliseconds.<br />Smaller groups = lower latency but more overhead.<br />Larger groups = higher latency but better efficiency.<br />Default: 40ms (2 Opus frames at 20ms each).<br />For real-time applications, use 20-60ms. For high-latency networks, use 100ms+.<br />min: `0` |
| `initial_delay_ms` | `integer (uint64)` | no | `0` | Adds a timestamp offset (playout delay) so receivers can buffer before playback.<br /><br />This is especially helpful when subscribers are on higher-latency / higher-jitter links,<br />and the client begins playback as soon as it sees the first frame.<br /><br />Default: 0 (no added delay).<br />min: `0` |
| `jwt` | `null | string` | no | `null` | Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.<br /><br />This is compatible with moq-relay and StreamKit's built-in MoQ auth. |
| `url` | `string` | no | — | — |
| `video` | `boolean | null` | no | `null` | Whether to publish a video track (VP9/AV1 on the `in_1` pin).<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup. In oneshot pipelines this is auto-detected from `input_types`<br />when left as `None`. |
| `video_codec` | `null | value` | no | `null` | Video codec for the MoQ catalog.<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup.  When `None`, the codec is auto-detected from `input_types`<br />(static pipelines) and falls back to VP9. |


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
  "properties": {
    "audio": {
      "default": null,
      "description": "Whether to publish an audio track (Opus on the `in` pin).\n\nRequired for dynamic pipelines where `input_types` is not available at\nstartup. In oneshot pipelines this is auto-detected from `input_types`\nwhen left as `None`.",
      "type": [
        "boolean",
        "null"
      ]
    },
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
      "description": "Audio codec for the MoQ catalog.\n\nRequired for dynamic pipelines where `input_types` is not available at\nstartup.  When `None`, the codec is auto-detected from `input_types`\n(static pipelines) and falls back to Opus."
    },
    "broadcast": {
      "default": "",
      "type": "string"
    },
    "channels": {
      "default": 2,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "group_duration_ms": {
      "default": 40,
      "description": "Duration of each MoQ group in milliseconds.\nSmaller groups = lower latency but more overhead.\nLarger groups = higher latency but better efficiency.\nDefault: 40ms (2 Opus frames at 20ms each).\nFor real-time applications, use 20-60ms. For high-latency networks, use 100ms+.",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "initial_delay_ms": {
      "default": 0,
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nThis is especially helpful when subscribers are on higher-latency / higher-jitter links,\nand the client begins playback as soon as it sees the first frame.\n\nDefault: 0 (no added delay).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "jwt": {
      "default": null,
      "description": "Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.\n\nThis is compatible with moq-relay and StreamKit's built-in MoQ auth.",
      "type": [
        "string",
        "null"
      ]
    },
    "url": {
      "default": "",
      "type": "string"
    },
    "video": {
      "default": null,
      "description": "Whether to publish a video track (VP9/AV1 on the `in_1` pin).\n\nRequired for dynamic pipelines where `input_types` is not available at\nstartup. In oneshot pipelines this is auto-detected from `input_types`\nwhen left as `None`.",
      "type": [
        "boolean",
        "null"
      ]
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
      "description": "Video codec for the MoQ catalog.\n\nRequired for dynamic pipelines where `input_types` is not available at\nstartup.  When `None`, the codec is auto-detected from `input_types`\n(static pipelines) and falls back to VP9."
    }
  },
  "title": "MoqPushConfig",
  "type": "object"
}
```

</details>
