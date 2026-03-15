---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Binary"
description: "PacketType Binary structure"
---

`PacketType` id: `Binary`

Type system: `PacketType::Binary`

Runtime: `Packet::Binary { data, content_type, metadata }`

## UI Metadata
- `label`: `Binary`
- `color`: `#45b7d1`
- `compat: exact, color: `#45b7d1``

## Structure
Binary packets are carried as:

```json
{
  "data": "<base64>",
  "content_type": "application/octet-stream",
  "metadata": { "timestamp_us": 0, "duration_us": 20000, "sequence": 42 }
}
```

Notes:

- `data` is base64-encoded for JSON transport.
- `content_type` is optional and may be `null`.
