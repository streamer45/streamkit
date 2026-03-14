---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::peer"
description: "Bidirectional MoQ peer for real-time media communication. Acts as both publisher and subscriber over a single WebTransport connection, supporting Opus audio and VP9 video."
---

`kind`: `transport::moq::peer`

Bidirectional MoQ peer for real-time media communication. Acts as both publisher and subscriber over a single WebTransport connection, supporting Opus audio and VP9 video.

## Categories
- `transport`
- `moq`
- `bidirectional`
- `dynamic`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)
- `in_1` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None }), EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
- `out` produces `Any` (broadcast)
- `out_1` produces `Any` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `allow_reconnect` | `boolean` | no | `false` | Allow publisher reconnections without recreating the session |
| `gateway_path` | `string` | no | `/moq` | Base path for gateway routing (e.g., "/moq")<br />Publishers connect to "{gateway_path}/input", subscribers to "{gateway_path}/output" |
| `input_broadcast` | `string` | no | `input` | Broadcast name to receive from publisher client |
| `output_broadcast` | `string` | no | `output` | Broadcast name to send to subscriber clients |
| `output_group_duration_ms` | `integer (uint64)` | no | `40` | Duration of each MoQ group in milliseconds for the subscriber output.<br /><br />Default: 40ms (2 Opus frames at 20ms each).<br />min: `0` |
| `output_initial_delay_ms` | `integer (uint64)` | no | `0` | Adds a timestamp offset (playout delay) so receivers can buffer before playback.<br /><br />Default: 0 (no added delay).<br />min: `0` |
| `video_height` | `integer (uint32)` | no | `480` | Video height in pixels for the MoQ catalog.<br />Used to advertise the video resolution to subscribers.<br />Default: 480.<br />min: `0` |
| `video_width` | `integer (uint32)` | no | `640` | Video width in pixels for the MoQ catalog.<br />Used to advertise the video resolution to subscribers.<br />Default: 640.<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "MoqPeerConfig",
  "type": "object",
  "properties": {
    "input_broadcast": {
      "description": "Broadcast name to receive from publisher client",
      "type": "string",
      "default": "input"
    },
    "output_broadcast": {
      "description": "Broadcast name to send to subscriber clients",
      "type": "string",
      "default": "output"
    },
    "gateway_path": {
      "description": "Base path for gateway routing (e.g., \"/moq\")\nPublishers connect to \"{gateway_path}/input\", subscribers to \"{gateway_path}/output\"",
      "type": "string",
      "default": "/moq"
    },
    "allow_reconnect": {
      "description": "Allow publisher reconnections without recreating the session",
      "type": "boolean",
      "default": false
    },
    "output_group_duration_ms": {
      "description": "Duration of each MoQ group in milliseconds for the subscriber output.\n\nDefault: 40ms (2 Opus frames at 20ms each).",
      "type": "integer",
      "format": "uint64",
      "minimum": 0,
      "default": 40
    },
    "output_initial_delay_ms": {
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nDefault: 0 (no added delay).",
      "type": "integer",
      "format": "uint64",
      "minimum": 0,
      "default": 0
    },
    "video_width": {
      "description": "Video width in pixels for the MoQ catalog.\nUsed to advertise the video resolution to subscribers.\nDefault: 640.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 640
    },
    "video_height": {
      "description": "Video height in pixels for the MoQ catalog.\nUsed to advertise the video resolution to subscribers.\nDefault: 480.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 480
    }
  }
}
```

</details>
