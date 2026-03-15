---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Packet Types
description: Supported packet types and their structures
---

StreamKit pipelines are **type-checked** using `PacketType` and exchange runtime data using `Packet`.

At runtime, the server also exposes UI-oriented packet metadata (labels, colors, compatibility rules):

```bash
curl http://localhost:4545/api/v1/schema/packets
```

## Types

| PacketType | Link | Runtime representation | Notes |
| --- | --- | --- | --- |
| `RawAudio` | [**Raw Audio**](./raw-audio/) | `Packet::Audio(AudioFrame)` | compat: wildcard fields (sample_rate, channels, sample_format), color: `#f39c12` |
| `EncodedAudio` | [**Encoded Audio**](./encoded-audio/) | `Packet::Binary { data, metadata, .. }` | compat: wildcard fields (codec, codec_private), color: `#ff6b6b` |
| `RawVideo` | [**Raw Video**](./raw-video/) | `Packet::Video(VideoFrame)` | compat: wildcard fields (width, height, pixel_format), color: `#1abc9c` |
| `EncodedVideo` | [**Encoded Video**](./encoded-video/) | `Packet::Binary { data, metadata, .. }` | compat: wildcard fields (codec, bitstream_format, codec_private, profile, level), color: `#2980b9` |
| `Text` | [**Text**](./text/) | `Packet::Text(Arc<str>)` | compat: exact, color: `#4ecdc4` |
| `Transcription` | [**Transcription**](./transcription/) | `Packet::Transcription(Arc<TranscriptionData>)` | compat: exact, color: `#9b59b6` |
| `Custom` | [**Custom**](./custom/) | `Packet::Custom(Arc<CustomPacketData>)` | compat: wildcard fields (type_id), color: `#e67e22` |
| `Binary` | [**Binary**](./binary/) | `Packet::Binary { data, content_type, metadata }` | compat: exact, color: `#45b7d1` |
| `Any` | [**Any**](./any/) | `Type-system only (matches any PacketType)` | compat: any, color: `#96ceb4` |
| `Passthrough` | [**Passthrough**](./passthrough/) | `Type inference marker (output type = input type)` | — |

## Serialization

`PacketType` serializes as:

- A string for unit variants (e.g., `"Text"`, `"Binary"`).
- An object for payload variants (e.g., `{"RawAudio": {"sample_rate": 48000, ...}}`).
