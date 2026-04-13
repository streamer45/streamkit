<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# UI Development Guide

The web UI lives in `ui/` and is a React 19 + TypeScript SPA built with Vite
and Bun.

## State Management

StreamKit uses three complementary state layers:

| Layer | Tool | Purpose |
|-------|------|---------|
| **Global session state** | Zustand (`ui/src/stores/sessionStore.ts`) | Pipeline, node states, stats, runtime schemas |
| **Atomic UI state** | Jotai (`ui/src/stores/`, various atoms) | Local UI concerns (selections, toggles, form state) |
| **Server state** | React Query (`@tanstack/react-query`) | REST API data (samples, marketplace, assets) |

### Critical Rule: WebSocket is Source of Truth

The session store is kept current by WebSocket event handlers in
`ui/src/services/websocket.ts`. This is the **only** race-free way to read
pipeline state, node states, stats, and runtime schemas.

**Never** intermix REST fetches with WebSocket state for the same data. REST
snapshots can be stale relative to WS events (e.g., `RuntimeSchemasUpdated`
arriving before or after a REST `fetchPipeline`).

## Component Patterns

- **Functional components** with hooks — no class components.
- **React.memo** for expensive components, especially in the compositor
  (see `agent_docs/render-performance.md`).
- **Radix UI** primitives for accessible UI components (dialogs, dropdowns,
  sliders, tabs, tooltips).
- **Lucide React** for icons.
- **Motion** (Framer Motion) for animations.

## Key Directories

```
ui/src/
├── components/    UI components (CompositorCanvas, NodeEditor, etc.)
├── hooks/         Custom React hooks (useCompositorLayers, useWebSocket, etc.)
├── views/         Top-level route views (Design, Monitor, Convert, Stream)
├── stores/        Zustand stores and Jotai atoms
├── services/      API clients and WebSocket handler
├── nodes/         React Flow custom node components
├── panes/         Panel/pane components for the editor layout
├── types/         TypeScript type definitions (auto-generated from Rust)
├── constants/     Shared constants
├── utils/         Utility functions
├── test/          Test utilities and perf measurement helpers
└── perf/          Dev-only React.Profiler wrappers
```

## Type Generation

TypeScript types are auto-generated from Rust structs using `ts-rs`. Run:

```bash
just gen-types
```

This produces types in `ui/src/types/` that match the Rust API contract. Do not
manually edit generated type files.

## Verification Commands

```bash
bun run lint          # prettier + eslint + tsc --noEmit
bun run test:run      # vitest (all unit tests)
bun run knip          # detect unused exports/dependencies
bun run build         # production bundle
```

Or via the justfile from the repo root:

```bash
just lint-ui          # same as bun run lint
just test-ui          # same as bun run test:run
just build-ui         # production build
just perf-ui          # render performance regression tests
```
