<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Engine

Pipeline execution engines for StreamKit.

## What Lives Here

- Dynamic session runtime (long-lived pipelines with control plane messages)
- Oneshot engine (HTTP batch processing for `/api/v1/process`)
- Backpressure/buffering coordination between nodes

Most “business logic” for specific packet types lives in `nodes`; the engine is responsible for wiring,
scheduling, and orchestration.
