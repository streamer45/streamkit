---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::peer"
description: "Bidirectional MoQ peer for real-time audio and video communication. Acts as both publisher and subscriber over a single WebTransport connection. Supported codecs: Opus (audio), VP9 (video)."
---

`kind`: `transport::moq::peer`

Bidirectional MoQ peer for real-time audio and video communication. Acts as both publisher and subscriber over a single WebTransport connection. Supported codecs: Opus (audio), VP9 (video).

## Categories
- `transport`
- `moq`
- `bidirectional`
- `dynamic`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)
- `in_1` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None }), EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
- `audio/data` produces `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None })` (broadcast)
- `video/data` produces `EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `allow_reconnect` | `boolean` | no | `false` | Allow publisher reconnections without recreating the session |
| `audio_codec` | `null | string enum[opus, aac]` | no | `null` | Audio codec for the MoQ catalog.<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup.  When `None`, the codec is auto-detected from `input_types`<br />(static pipelines) and falls back to Opus.<br /><br />Controls the **publisher output pin** type (`audio/data`).  For<br />transcoding scenarios where the subscriber receives a different codec<br />(e.g. Opus in → AAC out), use [`subscriber_audio_codec`] to override<br />the subscriber catalog codec independently. |
| `gateway_path` | `string` | no | `/moq` | Base path for gateway routing (e.g., "/moq")<br />Publishers connect to "{gateway_path}/input", subscribers to "{gateway_path}/output" |
| `input_broadcasts` | `array<string>` | no | `["input"]` | Broadcast names to receive from the publisher client.<br /><br />The first element is the primary broadcast (used for the dedicated<br />`/input` sub-path).  Additional elements are only supported via<br />bidirectional (base path) connections.  Output pins for tracks from<br />non-primary broadcasts are namespaced as<br />`{broadcast_name}/{track_name}` (e.g. `screen-input/video/hd`). |
| `output_broadcast` | `string` | no | `output` | Broadcast name to send to subscriber clients |
| `output_group_duration_ms` | `integer (uint64)` | no | `40` | Duration of each MoQ group in milliseconds for the subscriber output.<br /><br />Default: 40ms (2 Opus frames at 20ms each).<br />min: `0` |
| `output_initial_delay_ms` | `integer (uint64)` | no | `0` | Adds a timestamp offset (playout delay) so receivers can buffer before playback.<br /><br />Default: 0 (no added delay).<br />min: `0` |
| `subscriber_audio_codec` | `null | string enum[opus, aac]` | no | `null` | Audio codec advertised in the **subscriber** MoQ catalog.<br /><br />When set, overrides [`audio_codec`] for the subscriber side only<br />(catalog, frame duration).  The publisher output pin (`audio/data`)<br />continues to use [`audio_codec`].<br /><br />Useful for transcoding pipelines where the publisher sends one codec<br />(e.g. Opus) but the pipeline re-encodes to another (e.g. AAC) before<br />feeding it back to subscribers.<br /><br />When `None`, falls back to [`audio_codec`]. |
| `video_codec` | `null | value` | no | `null` | Video codec for the MoQ catalog.<br /><br />Required for dynamic pipelines where `input_types` is not available at<br />startup.  When `None`, the codec is auto-detected from `input_types`<br />(static pipelines) and falls back to VP9. |
| `video_height` | `integer (uint32)` | no | `480` | Video height in pixels for the MoQ catalog.<br />Used to advertise the video resolution to subscribers.<br />Default: 480.<br />min: `0` |
| `video_width` | `integer (uint32)` | no | `640` | Video width in pixels for the MoQ catalog.<br />Used to advertise the video resolution to subscribers.<br />Default: 640.<br />min: `0` |


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
    "allow_reconnect": {
      "default": false,
      "description": "Allow publisher reconnections without recreating the session",
      "type": "boolean"
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
      "description": "Audio codec for the MoQ catalog.\n\nRequired for dynamic pipelines where `input_types` is not available at\nstartup.  When `None`, the codec is auto-detected from `input_types`\n(static pipelines) and falls back to Opus.\n\nControls the **publisher output pin** type (`audio/data`).  For\ntranscoding scenarios where the subscriber receives a different codec\n(e.g. Opus in → AAC out), use [`subscriber_audio_codec`] to override\nthe subscriber catalog codec independently."
    },
    "gateway_path": {
      "default": "/moq",
      "description": "Base path for gateway routing (e.g., \"/moq\")\nPublishers connect to \"{gateway_path}/input\", subscribers to \"{gateway_path}/output\"",
      "type": "string"
    },
    "input_broadcasts": {
      "default": [
        "input"
      ],
      "description": "Broadcast names to receive from the publisher client.\n\nThe first element is the primary broadcast (used for the dedicated\n`/input` sub-path).  Additional elements are only supported via\nbidirectional (base path) connections.  Output pins for tracks from\nnon-primary broadcasts are namespaced as\n`{broadcast_name}/{track_name}` (e.g. `screen-input/video/hd`).",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "output_broadcast": {
      "default": "output",
      "description": "Broadcast name to send to subscriber clients",
      "type": "string"
    },
    "output_group_duration_ms": {
      "default": 40,
      "description": "Duration of each MoQ group in milliseconds for the subscriber output.\n\nDefault: 40ms (2 Opus frames at 20ms each).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "output_initial_delay_ms": {
      "default": 0,
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nDefault: 0 (no added delay).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "subscriber_audio_codec": {
      "anyOf": [
        {
          "$ref": "#/$defs/AudioCodec"
        },
        {
          "type": "null"
        }
      ],
      "default": null,
      "description": "Audio codec advertised in the **subscriber** MoQ catalog.\n\nWhen set, overrides [`audio_codec`] for the subscriber side only\n(catalog, frame duration).  The publisher output pin (`audio/data`)\ncontinues to use [`audio_codec`].\n\nUseful for transcoding pipelines where the publisher sends one codec\n(e.g. Opus) but the pipeline re-encodes to another (e.g. AAC) before\nfeeding it back to subscribers.\n\nWhen `None`, falls back to [`audio_codec`]."
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
    },
    "video_height": {
      "default": 480,
      "description": "Video height in pixels for the MoQ catalog.\nUsed to advertise the video resolution to subscribers.\nDefault: 480.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "video_width": {
      "default": 640,
      "description": "Video width in pixels for the MoQ catalog.\nUsed to advertise the video resolution to subscribers.\nDefault: 640.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "MoqPeerConfig",
  "type": "object"
}
```

</details>
