---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::rtmp::publish"
description: "Publishes encoded H.264 video and AAC audio to an RTMP endpoint. Accepts Annex B H.264 on the 'video' pin and raw AAC frames on the 'audio' pin, converting to the RTMP/FLV wire format. Supports both RTMP and RTMPS (TLS)."
---

`kind`: `transport::rtmp::publish`

Publishes encoded H.264 video and AAC audio to an RTMP endpoint. Accepts Annex B H.264 on the 'video' pin and raw AAC frames on the 'audio' pin, converting to the RTMP/FLV wire format. Supports both RTMP and RTMPS (TLS).

## Categories
- `transport`
- `rtmp`

## Pins
### Inputs
- `video` accepts `EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)
- `audio` accepts `EncodedAudio(EncodedAudioFormat { codec: Aac, codec_private: None })` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint8)` | no | `2` | Number of audio channels for the AAC sequence header.<br /><br />Must match the channel count produced by the upstream AAC encoder.<br />1 = mono, 2 = stereo.<br />Defaults to 2 (stereo).<br />min: `0`<br />max: `255` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz for the AAC sequence header.<br /><br />Must match the sample rate produced by the upstream AAC encoder.<br />Common values: 48000, 44100, 32000.<br />Defaults to 48000.<br />min: `0` |
| `stream_key` | `null | string` | no | `null` | Stream key appended to the URL path.<br /><br />Optional — if omitted, the URL is used as-is (for URLs that<br />already include the key).  Ignored when `stream_key_env` is set. |
| `stream_key_env` | `null | string` | no | `null` | Environment variable name containing the stream key.<br /><br />Read at node startup.  Takes precedence over `stream_key`.<br />The name is fully user-controlled, so multiple RTMP output nodes<br />can each reference different variables.<br /><br />Example: `"SKIT_RTMP_STREAM_KEY"` → reads `$SKIT_RTMP_STREAM_KEY`. |
| `url` | `string` | yes | — | RTMP server URL.<br /><br />Supports `rtmp://` and `rtmps://` (TLS) schemes.<br />Can include the stream key in the path, or use the separate<br />`stream_key` / `stream_key_env` fields.<br /><br />Examples:<br />- `rtmp://a.rtmp.youtube.com/live2` (key via `stream_key` or `stream_key_env`)<br />- `rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx` (key inline)<br />- `rtmps://live.twitch.tv/app/live_xxxx` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the RTMP publisher node.",
  "properties": {
    "channels": {
      "default": 2,
      "description": "Number of audio channels for the AAC sequence header.\n\nMust match the channel count produced by the upstream AAC encoder.\n1 = mono, 2 = stereo.\nDefaults to 2 (stereo).",
      "format": "uint8",
      "maximum": 255,
      "minimum": 0,
      "type": "integer"
    },
    "sample_rate": {
      "default": 48000,
      "description": "Audio sample rate in Hz for the AAC sequence header.\n\nMust match the sample rate produced by the upstream AAC encoder.\nCommon values: 48000, 44100, 32000.\nDefaults to 48000.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "stream_key": {
      "default": null,
      "description": "Stream key appended to the URL path.\n\nOptional — if omitted, the URL is used as-is (for URLs that\nalready include the key).  Ignored when `stream_key_env` is set.",
      "type": [
        "string",
        "null"
      ]
    },
    "stream_key_env": {
      "default": null,
      "description": "Environment variable name containing the stream key.\n\nRead at node startup.  Takes precedence over `stream_key`.\nThe name is fully user-controlled, so multiple RTMP output nodes\ncan each reference different variables.\n\nExample: `\"SKIT_RTMP_STREAM_KEY\"` → reads `$SKIT_RTMP_STREAM_KEY`.",
      "type": [
        "string",
        "null"
      ]
    },
    "url": {
      "description": "RTMP server URL.\n\nSupports `rtmp://` and `rtmps://` (TLS) schemes.\nCan include the stream key in the path, or use the separate\n`stream_key` / `stream_key_env` fields.\n\nExamples:\n- `rtmp://a.rtmp.youtube.com/live2` (key via `stream_key` or `stream_key_env`)\n- `rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx` (key inline)\n- `rtmps://live.twitch.tv/app/live_xxxx`",
      "type": "string"
    }
  },
  "required": [
    "url"
  ],
  "title": "RtmpPublishConfig",
  "type": "object"
}
```

</details>
