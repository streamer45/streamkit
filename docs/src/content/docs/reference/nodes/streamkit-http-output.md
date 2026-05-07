---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "streamkit::http_output"
description: "Synthetic output node for oneshot HTTP pipelines. Sends binary data as the HTTP response body."
---

`kind`: `streamkit::http_output`

Synthetic output node for oneshot HTTP pipelines. Sends binary data as the HTTP response body.

> [!NOTE]
> This is a **synthetic node** — it has no runtime implementation. The oneshot engine collects its input and streams it back as the HTTP response. It can only be used in `mode: oneshot` pipelines (via `POST /api/v1/process`), not in dynamic sessions.

## Categories
- `transport`
- `oneshot`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
No outputs.

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{}
```

</details>
