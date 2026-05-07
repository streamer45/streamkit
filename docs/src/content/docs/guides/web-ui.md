---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Using the Web UI
description: Guide to the StreamKit visual pipeline editor
---

StreamKit includes a React-based web interface for building and monitoring pipelines visually.

## Accessing the UI

Open [http://localhost:4545](http://localhost:4545) after starting the server:

```bash
just skit serve
# or
just dev  # For development with hot reload
```

If auth is enabled, you’ll be redirected to `/login`. Print the bootstrap admin token and paste it into the UI:

```bash
skit auth print-admin-token
```

See the [Authentication guide](/guides/authentication/) for details.

## Finding Build Info

Click the StreamKit logo in the top-left corner to open the About modal. It shows the server
version and build hash (commit) for debugging or support. The same data is available from the
`/healthz` endpoint.

## Main Routes

The Web UI has four main routes:

- **Design** (default): build and edit pipelines visually.
- **Monitor**: inspect and manage live sessions.
- **Convert**: demo/test **oneshot** pipelines (request → response).
- **Stream**: demo/test **dynamic** pipelines (MOQ-powered streaming sessions).

## Design View

Design View is the default route and is split into three panes:

- **Left pane**: library and tools:
  - **Nodes**: the node palette/library (built-ins + loaded plugins).
  - **Plugins**: view/manage loaded plugins and marketplace installs (availability depends on your role/config).
  - **Samples**: example pipelines you can load as a starting point.
  - **Fragments**: reusable mini-graphs (pre-wired sets of nodes) you can drop into a larger pipeline to share common patterns.
- **Center pane (canvas)**: a React Flow editor where you:
  - Drag and drop nodes onto the canvas.
  - Connect nodes by drawing edges between ports.
  - Use the right-click context menu for actions like import/export.
- **Right pane**:
  - **YAML**: the pipeline definition for the canvas (two-way synced).
  - **Inspector** (when a node is selected): inspect/tune that node’s parameters.

## Monitor View

Monitor View uses the same overall three-pane layout, but focuses on running sessions:

- **Left pane**: a live list of sessions until you enter **Staging Mode** (then it switches to the node library/palette for editing).
- **Center pane**: the session graph view. If the selected session contains a `video::compositor` node, a **compositor scene editor** is available — an interactive canvas where you can drag, resize, reorder, and configure video layers and overlays in real time.
- **Right pane** (once a session is selected): the YAML editor plus the Inspector pane for selected nodes.

## Convert View

Convert is for demoing/testing **oneshot** pipelines: load or author a pipeline, run it against input media, and review outputs.

A reliable first run is the bundled audio-mixing template:

1. Open `/convert`.
2. Select **Audio Mixing (Upload + Music Track)**.
3. Upload `samples/audio/system/sample.ogg` (or switch to **Select Existing Asset** and choose a bundled audio asset).
4. Click **Convert File**.
5. Wait for **Converted Audio** and play/download the output.

Plugin-backed templates need their plugin and model files installed first. If a template references `plugin::native::*`, build/install that plugin before using the template.

## Stream View

Stream is for demoing/testing **dynamic** (long-running) pipelines using MoQ-powered streaming sessions.

For local source builds, start the backend and hot-reload UI separately so the browser uses the current UI code and a local MoQ gateway URL:

```bash
SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq SK_SERVER__ADDRESS=127.0.0.1:4545 just skit
just ui
```

Then open `http://localhost:3045/stream`. A reliable first run is:

1. Select **Video Color Bars (MoQ Stream)**.
2. Click **Create Session**.
3. Wait for **Session Active**.
4. If it does not auto-connect, click **Connect & Stream**.
5. Wait for **Relay: connected** and **Watch: live**, then verify the color-bars canvas is rendering.

No HTTPS/TLS setup is required for local MoQ testing with the Vite dev UI.

## Exporting Pipelines

In Design View, use the canvas right-click context menu to export/import pipelines. The YAML pane is also a convenient way to copy/paste pipelines for sharing/versioning.

To run exported YAML without the UI, send it to the server:

- Dynamic session: `POST /api/v1/sessions` with JSON `{ "name": "...", "yaml": "..." }`
- Oneshot: `POST /api/v1/process` with multipart field `config` (and optional `media`)

## Next Steps

- [Creating Pipelines](/guides/creating-pipelines/) - YAML syntax reference
- [Writing Plugins](/guides/writing-plugins/) - Extend with custom nodes
