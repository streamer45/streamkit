<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Common Agent Pitfalls

Known mistakes that coding agents frequently make in this codebase. Read this
before starting work to avoid wasted effort and review cycles.

## UI State: Always Use WebSocket-Driven State

**Never** intermix REST fetches with WebSocket state for pipeline, node, or
runtime data. The session store (`ui/src/stores/sessionStore.ts`) is kept
current by WS event handlers in `ui/src/services/websocket.ts` and is the
**only** race-free way to read pipeline state.

REST snapshots can be stale relative to WS events (e.g., `RuntimeSchemasUpdated`
arriving before or after a REST `fetchPipeline`). If you need pipeline data in a
component, read it from the Zustand session store — never fetch it separately.

## Never Commit `perf-baselines.json` Changes

The `perf-baselines.json` file at the repo root should **only** be committed by
human maintainers who have benchmarked on bare-metal idle systems. If `just
perf-ui` modifies the file, revert it before committing:

```bash
git checkout -- perf-baselines.json
```

## UI Tooling: Bun Only

Use `bun install`, `bunx`, and `bun run` for all UI work. **Never** use npm or
pnpm — they are not supported and will produce incompatible lockfiles.

## SPDX License Headers Required

All new files need SPDX license headers. The format varies by language:

```rust
// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0
```

```typescript
// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0
```

```html
<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->
```

```yaml
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0
```

## Lint Suppressions Need Justification

Do **not** blindly suppress lint warnings with `#[allow(...)]`, `eslint-disable`,
or similar. Instead, refactor or improve the code to address the underlying issue.
If a suppression is truly necessary, it **must** include a comment explaining the
rationale.

## UI Must Be Built Before Compiling the Server

The skit server embeds the web UI at compile time via RustEmbed. You must run
`just build-ui` before `cargo build` for the server crate, or compilation will
fail with a missing `ui/dist/` error.

## Conventional Commits with DCO Sign-Off

All commits must use Conventional Commit format and include DCO sign-off:

```bash
git commit -s -m "feat(nodes): add MP3 decoder"
```

Types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`, `ci`, `build`

Scopes: `core`, `api`, `engine`, `nodes`, `server`, `client`, `ui`, `plugins`

## Avoid `unwrap()` in Production Rust Code

Use `Result` types and proper error handling. `unwrap()` is only acceptable in
tests.

## React.memo Barriers in Compositor Components

The compositor UI uses `React.memo` extensively to prevent cascade re-renders.
When modifying compositor hooks or components (`useCompositorLayers`,
`CompositorNode`, etc.), run `just perf-ui` afterward to verify you haven't
broken memoization barriers. See `agent_docs/render-performance.md` for details.
