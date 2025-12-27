<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Nodes

Built-in processing nodes for StreamKit pipelines.

## What Lives Here

- Built-in node implementations (e.g. `core::*`, `audio::*`, `containers::*`, `transport::*`)
- Node parameter schemas (used by the UI for validation and editor controls)
- Node-level tests and fixtures

If you’re adding a new built-in node, the usual flow is:

1. Implement the node in the appropriate module
2. Register it in `streamkit_nodes::register_nodes`
3. Add/update docs via `just gen-docs-reference`
