---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Text"
description: "PacketType Text structure"
---

`PacketType` id: `Text`

Type system: `PacketType::Text`

Runtime: `Packet::Text(Arc<str>)`

## UI Metadata
- `label`: `Text`
- `color`: `#4ecdc4`
- `compat: exact, color: `#4ecdc4``

## Structure
Text packets are carried as `Packet::Text(Arc<str>)`.
