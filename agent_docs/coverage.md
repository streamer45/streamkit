<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Testing & Coverage Guidelines

Coverage is a means, not an end. The goal is **durable confidence** in the
parts of the system most likely to break or hurt users when they do — not a
number to optimise.

## Philosophy

A good industry target is **70–85% overall coverage**, with higher
expectations for core business logic and lower expectations for UI glue
code, generated code, configuration, or thin framework wrappers.

What gets covered matters more than the percentage:

- **Critical business rules** — heavily tested.
- **Complex branching logic** — strong branch coverage.
- **Bug-prone areas** — regression tests for every bug fix.
- **Public APIs and shared libraries** — higher bar.
- **Simple boilerplate and thin wrappers** — minimal or no extra tests.

## Practical standard

> Aim for **80%+ coverage on meaningful code**, but require new or changed
> code to be well-tested rather than obsessing over total project coverage.

For StreamKit specifically:

| Surface | Target |
|---------|--------|
| Patch coverage (new / changed lines in a PR) | **≥ 80%** |
| Hot-path subsystems (`crates/core`, `crates/engine`, `crates/api`, `apps/skit/src/server/*`) | **80–90%** |
| Total project coverage (per `backend` / `ui` flag) | **≥ 70%** and trending up |
| Generated code, examples, vendored / thin wrappers | excluded from coverage (see `codecov.yml`'s `ignore` block) |

The `codecov.yml` ratchet plan lives in that file's header comment.
The project flags (`backend`, `ui`) now report red on any drop > 1% and
`patch.default` reports red when new / changed code is below 80%.
These thresholds are **enforced by the merge gate**: the `all-checks`
job in `ci.yml` polls Codecov check runs and fails the required check when
any threshold is violated. The coverage jobs themselves keep
`continue-on-error: true` so a flaky coverage toolchain doesn't
independently block merges — only Codecov's own threshold verdict
matters. If Codecov check runs don't appear within the polling window
(~5 min), the gate degrades gracefully with a warning rather than
blocking. Component-level statuses remain informational while individual
subsystems settle near the 80% target.

## What to do — and not do — when adding tests

**Do:**

- Verify real, observable behaviour (inputs → outputs, public APIs,
  user-visible outcomes).
- Cover meaningful success paths, failure paths, and important boundary
  cases.
- Prefer table-driven tests, factories, fixtures, and shared helpers over
  copy-paste.
- Use a lightweight real interaction (an in-memory fixture, a real
  filesystem in a `tempfile::TempDir`, a real `tokio::test` runtime) when
  it gives better confidence than a heavy mock.
- Follow the existing testing style in the same module — look for an
  adjacent `mod tests` or `*.test.ts` before inventing a new pattern.

**Don't:**

- Don't add superficial tests that execute code without asserting
  behaviour just to move the percentage.
- Don't over-test private implementation details or obscure edge cases
  solely to inflate coverage.
- Don't chase 100% — diminishing returns past ~80% are real, and the
  marginal tests are usually the most brittle.
- Don't mock excessively when a lightweight real interaction is cheaper
  and gives better signal.
- Don't introduce assertions tied to internal data shapes that aren't
  part of any stable contract — those tests break on every refactor.

## When you spot a bug while writing tests

Test-only PRs should stay test-only. If a test surfaces a real
production bug:

1. Note it in the PR description under "Follow-ups / observations".
2. Pin the current (broken) behaviour with a test, with an inline
   comment pointing at the bug, so the test starts failing when the
   fix lands.
3. Open a tracking issue if it won't be addressed immediately.

Don't bundle a fix into a test-only PR — it makes the review of both
harder.

## Commands

| Task | Command |
|------|---------|
| Backend coverage (HTML + lcov) | `just cov-skit` |
| UI coverage (HTML + lcov) | `cd ui && bun run test:coverage` |
| Both | `just cov` |
| Backend tests only | `cargo test --workspace` |
| UI tests only | `just test-ui` |

Reports land under `target/coverage/html/` (backend) and
`ui/coverage/lcov-report/` (UI). Open `index.html` in either to drill
down per file.

## Codecov configuration

See [`codecov.yml`](../codecov.yml) for the live config, including the
component breakdown, flag rules, the `ignore` list, and the phased
ratchet plan. The dashboard is at
<https://app.codecov.io/gh/streamer45/streamkit>.
