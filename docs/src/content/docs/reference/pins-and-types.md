---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Pins & Type Inference
description: How StreamKit validates connections (pin cardinality, packet types, and passthrough inference)
---

StreamKit nodes declare **input pins** and **output pins**. Pins are how the engine validates that a graph is connectable before it runs.

## Pins

- **Input pins** declare which packet types they accept (`accepts_types`).
- **Output pins** declare the single packet type they produce (`produces_type`).
- Most nodes use the conventional pin names `in` and `out`, but nodes may expose multiple pins.

## Pin Cardinality (Connection Multiplicity)

Pins also declare a **cardinality**, which controls how many connections are allowed:

- `one`: exactly one connection allowed.
- `broadcast` (outputs only): multiple connections allowed; the same packet is cloned and sent to all downstream connections.
- `dynamic` (usually inputs): a template for a family of pins created on demand (e.g. prefix `in` -> `in_0`, `in_1`, ...). This is common for nodes like mixers/routers.

In dynamic sessions, when you connect to a pin name that matches a dynamic family (like `in_3`), the engine can create that pin at runtime.

## Type Checking (What Can Connect)

When you connect `from_node.from_pin -> to_node.to_pin`, StreamKit checks that the output type is compatible with (at least one of) the destination pin's accepted types.

Compatibility rules are defined by the packet type registry:

- `Any` matches anything.
- Otherwise, the packet kind must match.
- Some packet types support structured compatibility (e.g. "match these fields, allow wildcards").

## Passthrough Types and Inference

Some nodes use the special packet type `Passthrough` to mean "I forward whatever type I receive" (common for generic transforms, script-like nodes, or adapters).

- **Oneshot pipelines** must fully resolve `Passthrough` types at build time so the engine can validate every connection. The oneshot graph builder attempts a simple inference pass by walking upstream connections and propagating concrete types through passthrough outputs.
- **Dynamic sessions** allow more permissive validation: passthrough connections are allowed and resolved at runtime based on the actual packets flowing through the graph.

If you hit a "Passthrough could not be resolved" or "incompatible connection" error in oneshot mode, make the types concrete (or insert a node that produces a concrete packet type) so the engine can validate the link.

## Related Docs

- [Creating Pipelines](/guides/creating-pipelines/) - YAML formats and `needs` connection shorthand
- [Packet Reference](/reference/packets/) - Packet types and metadata
