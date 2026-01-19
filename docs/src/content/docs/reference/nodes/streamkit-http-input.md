---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "streamkit::http_input"
description: "Synthetic input node for oneshot HTTP pipelines. Receives binary data from the HTTP request body."
---

`kind`: `streamkit::http_input`

Synthetic input node for oneshot HTTP pipelines. Receives binary data from the HTTP request body.

## Categories
- `transport`
- `oneshot`

## Pins
### Inputs
No inputs.

### Outputs
- Single-field mode: one `Binary` pin named after `field` (defaults to `media` when a single `http_input` exists).
- Multi-field mode: one `Binary` pin per `fields` entry. Pin names match the field names and **no legacy `media` pin is added**.

## Parameters
- `field` (`string`, optional) — Multipart field name to bind to this input. Defaults to `media` when there is only one `http_input` node; otherwise defaults to the node id.
- `fields` (`array`, optional) — List of multipart fields for this node. Each entry can be a string or `{ name, required }`. When set, only these fields are accepted and the legacy `media` field is disabled. `field` and `fields` are mutually exclusive.
- `required` (`boolean`, default: `true`) — When `true`, the request must include this field. Ignored when `fields` is provided (use per-entry `required` instead).

When `fields` is provided, this node exposes multiple output pins, one per field. Pin names match the field names, allowing you to wire each uploaded stream independently. The legacy `media` pin is not added in this mode.


<details>
<summary>Raw JSON Schema</summary>

```json
{}
```

</details>
