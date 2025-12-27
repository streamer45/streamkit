<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Core

Core traits and data structures used across StreamKit crates.

## What Lives Here

- Core packet types and metadata (`src/types.rs`, `src/packet_meta.rs`)
- Node interfaces and registry types (`src/registry.rs`)
- Shared resource management (`src/resource_manager.rs`)

This crate should stay dependency-light and focused on stable primitives; higher-level behavior belongs in
`engine`, `nodes`, or `server`.
