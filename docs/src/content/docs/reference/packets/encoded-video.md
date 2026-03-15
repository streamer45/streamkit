---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Encoded Video"
description: "PacketType EncodedVideo structure"
---

`PacketType` id: `EncodedVideo`

Type system: `PacketType::EncodedVideo(EncodedVideoFormat)`

Runtime: `Packet::Binary { data, metadata, .. }`

## UI Metadata
- `label`: `Encoded Video`
- `color`: `#2980b9`
- `display_template`: `Encoded Video ({codec})`
- `compat: wildcard fields (codec, bitstream_format, codec_private, profile, level), color: `#2980b9``

## Structure
Encoded video is defined by an `EncodedVideoFormat` (codec, bitstream format, profile, level)
in the type system.

At runtime, encoded video frames are carried as `Packet::Binary { data, metadata, .. }`. The codec nodes
encode/decode using this binary payload and label pins with the appropriate `EncodedVideo` type.
