---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Encoded Audio"
description: "PacketType EncodedAudio structure"
---

`PacketType` id: `EncodedAudio`

Type system: `PacketType::EncodedAudio(EncodedAudioFormat)`

Runtime: `Packet::Binary { data, metadata, .. }`

## UI Metadata
- `label`: `Encoded Audio`
- `color`: `#ff6b6b`
- `display_template`: `Encoded Audio ({codec})`
- `compat: wildcard fields (codec, codec_private), color: `#ff6b6b``

## Structure
Encoded audio is defined by an `EncodedAudioFormat` (codec + optional codec-private data) in the type system.

At runtime, encoded audio frames are carried as `Packet::Binary { data, metadata, .. }`. The codec nodes
encode/decode using this binary payload and label pins with the appropriate `EncodedAudio` type.
