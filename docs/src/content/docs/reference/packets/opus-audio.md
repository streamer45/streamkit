---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Opus Audio"
description: "PacketType OpusAudio structure"
---

`PacketType` id: `OpusAudio`

Type system: `PacketType::OpusAudio`

Runtime: `Packet::Binary { data, metadata, .. }`

## UI Metadata
- `label`: `Opus Audio`
- `color`: `#ff6b6b`
- `compat: exact, color: `#ff6b6b``

## Structure
Opus packets use the `OpusAudio` packet type, but the runtime payload is still `Packet::Binary`.

The Opus codec nodes encode/decode using `Packet::Binary { data, metadata, .. }` and label pins as `OpusAudio`.
