#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# Run Layer 2 (Playwright + React.Profiler) perf tests in CI.
#
# Starts the skit backend and Vite dev server, waits for both to be healthy,
# then runs the compositor and monitor perf specs against the dev server.
#
# Usage: .github/scripts/run-perf-e2e.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

SKIT_PID=""
VITE_PID=""

cleanup() {
  [ -n "$SKIT_PID" ] && kill "$SKIT_PID" 2>/dev/null || true
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Start skit backend on default port for Vite proxy
SK_SERVER__ADDRESS=127.0.0.1:4545 \
SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq \
SK_LOG__FILE_ENABLE=false \
RUST_LOG=warn \
"$REPO_ROOT/target/debug/skit" serve &
SKIT_PID=$!

# Start Vite dev server.
# Use `bun run dev` so Bun resolves from the project's package.json scripts.
cd "$REPO_ROOT/ui"
bun run dev &
VITE_PID=$!
cd "$REPO_ROOT"

# Wait for skit to become healthy
HEALTHY=0
for i in $(seq 1 30); do
  if curl -sf http://127.0.0.1:4545/healthz > /dev/null 2>&1; then
    echo "skit is healthy"
    HEALTHY=1
    break
  fi
  sleep 1
done
if [ "$HEALTHY" -ne 1 ]; then
  echo "ERROR: skit did not become healthy within 30s"
  exit 1
fi

# Wait for Vite dev server to be ready (60s — first start may pre-bundle deps)
HEALTHY=0
for i in $(seq 1 60); do
  if curl -sf http://127.0.0.1:3045/ > /dev/null 2>&1; then
    echo "Vite dev server is ready"
    HEALTHY=1
    break
  fi
  sleep 1
done
if [ "$HEALTHY" -ne 1 ]; then
  echo "ERROR: Vite dev server did not become ready within 60s"
  exit 1
fi

# Run perf tests against the dev server
cd "$REPO_ROOT/e2e"
E2E_BASE_URL=http://localhost:3045 bunx playwright test \
  tests/compositor-perf.spec.ts \
  tests/monitor-session-load-perf.spec.ts
