---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::mixer"
description: "Combines multiple audio streams into a single output by summing samples. Supports configurable number of input channels with per-channel gain control."
---

`kind`: `audio::mixer`

Combines multiple audio streams into a single output by summing samples. Supports configurable number of input channels with per-channel gain control.

## Categories
- `audio`
- `filters`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (dynamic)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `clocked` | `null | object` | no | — | Enable clocked mixing mode (dedicated mixing thread + per-input jitter buffers).<br /><br />When enabled, the mixer emits frames on a fixed cadence determined by<br />`sample_rate` and `frame_samples_per_channel`. |
| `num_inputs` | `integer | null (uint)` | no | `null` | Number of input pins to pre-create.<br />Required for stateless/oneshot pipelines where pins must exist before graph building.<br />Optional for dynamic pipelines where pins are created on-demand.<br />If specified, pins will be named in_0, in_1, ..., in_{N-1}.<br />min: `0` |
| `sync_timeout_ms` | `integer | null (uint64)` | no | `100` | Timeout in milliseconds for waiting for slow inputs.<br />If specified, the mixer will wait up to this duration for all active pins to provide frames.<br />If timeout expires, missing pins will be mixed as silence.<br />If not specified (None), the mixer will wait indefinitely (strict broadcast synchronization).<br />Default: Some(100)<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "AudioMixerConfig",
  "description": "Configuration for the AudioMixerNode.",
  "type": "object",
  "properties": {
    "sync_timeout_ms": {
      "description": "Timeout in milliseconds for waiting for slow inputs.\nIf specified, the mixer will wait up to this duration for all active pins to provide frames.\nIf timeout expires, missing pins will be mixed as silence.\nIf not specified (None), the mixer will wait indefinitely (strict broadcast synchronization).\nDefault: Some(100)",
      "type": [
        "integer",
        "null"
      ],
      "format": "uint64",
      "minimum": 0,
      "default": 100
    },
    "num_inputs": {
      "description": "Number of input pins to pre-create.\nRequired for stateless/oneshot pipelines where pins must exist before graph building.\nOptional for dynamic pipelines where pins are created on-demand.\nIf specified, pins will be named in_0, in_1, ..., in_{N-1}.",
      "type": [
        "integer",
        "null"
      ],
      "format": "uint",
      "minimum": 0,
      "default": null
    },
    "clocked": {
      "description": "Enable clocked mixing mode (dedicated mixing thread + per-input jitter buffers).\n\nWhen enabled, the mixer emits frames on a fixed cadence determined by\n`sample_rate` and `frame_samples_per_channel`.",
      "anyOf": [
        {
          "$ref": "#/$defs/ClockedMixerConfig"
        },
        {
          "type": "null"
        }
      ]
    }
  },
  "$defs": {
    "ClockedMixerConfig": {
      "type": "object",
      "properties": {
        "sample_rate": {
          "description": "Output sample rate (Hz). Inputs are expected to already match this.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0,
          "default": 48000
        },
        "frame_samples_per_channel": {
          "description": "Fixed frame size (samples per channel) for the clocked mixer.\n\nExample: `960` @ `48000` Hz => 20ms frames.",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 960
        },
        "jitter_buffer_frames": {
          "description": "Per-input jitter buffer depth (in frames).\n\nFrames are queued in order. When full, the oldest frame is dropped (overwrite-oldest).\n\nRecommended: 2-3 for ~40-60ms jitter tolerance at 20ms frames.",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 3
        },
        "generate_silence": {
          "description": "If true, emit silence frames on ticks even when no inputs have data.\n\nIf false, the clocked mixer only emits output on ticks where at least one input\ncontributes a frame.",
          "type": "boolean",
          "default": true
        }
      }
    }
  }
}
```

</details>
