<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Contributing to StreamKit

## Quick Start

```bash
git clone https://github.com/streamer45/streamkit.git
cd streamkit
cd ui && bun install && cd ..
just dev    # starts backend + frontend with hot reload
```

**Prerequisites:** Rust 1.92+, Bun 1.3+, [Just](https://github.com/casey/just) (recommended)

Run `just --list` to see all available commands.

## Making Changes

1. Create a branch: `git checkout -b feat/my-feature` or `fix/my-bug`
2. Make your changes
3. Run `just test` and `just lint`
4. Commit and push
5. Open a PR

## Commits

**All commits must be signed off** to certify you have the right to submit the code ([DCO](https://developercertificate.org/)):

```bash
git commit -s -m "feat(nodes): add MP3 decoder"
```

This adds a `Signed-off-by: Your Name <email>` line. The DCO check will fail on PRs without it.

We use [Conventional Commits](https://www.conventionalcommits.org/). Format:

```
type(scope): description
```

**Types:** `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`, `ci`, `build`

**Scopes:** `core`, `api`, `engine`, `nodes`, `server`, `client`, `ui`, `plugins`

Examples:
```
feat(nodes): add MP3 decoder
fix(engine): prevent panic on empty input
docs: update README
```

There's a warning-only commit hook - it won't block you, just nudges you toward the convention.

## Code Style

**Rust:**
- `cargo fmt` for formatting
- Fix all `cargo clippy` warnings
- Use `Result` types, avoid `unwrap()` in production code
- Add doc comments for public APIs

**TypeScript:**
- ESLint handles formatting
- Avoid `any` - use proper types
- Functional components with hooks
- Zustand for global state, React Query for server state

**All files** need SPDX license headers:
```rust
// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0
```

## Pull Requests

- Keep PRs focused (one feature/fix per PR)
- Add tests for new functionality
- Update docs if behavior changes
- Use conventional commit format for PR titles (they become squash-merge commits)
- CI must pass: tests, formatting, clippy, TypeScript compilation, license headers

## Plugins

**Native plugins** (fast, no sandbox): See `examples/plugins/gain-native/`

**WASM plugins** (sandboxed, cross-language): See `examples/plugins/gain-wasm-rust/` or `gain-wasm-go/`

## License

Contributions are licensed under [MPL-2.0](LICENSE).

## Help

- Discord: https://discord.gg/dcvxCzay47
- Questions: Open an issue or use GitHub Discussions
