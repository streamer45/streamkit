---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Passthrough"
description: "PacketType Passthrough structure"
---

`PacketType` id: `Passthrough`

Type system: `PacketType::Passthrough`

Runtime: `Type inference marker (output type = input type)`

## Structure
`Passthrough` is a type inference marker (output type = input type).

In oneshot pipelines it is resolved during compilation. In dynamic pipelines it may be resolved when packets flow.
