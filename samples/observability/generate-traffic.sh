#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# Drives TTS + STT traffic so the dashboards have data. By default it calls
# skit's oneshot /api/v1/process directly (no gateway required). Pass --gateway
# to route through the speech gateway instead (requires the `gateway` profile),
# which also populates the Speech Gateway dashboard row.
set -euo pipefail

ROUNDS="${ROUNDS:-20}"
SKIT_URL="${SKIT_URL:-http://localhost:4545}"
GATEWAY_URL="${GATEWAY_URL:-http://localhost:8080}"
HERE="$(cd "$(dirname "$0")" && pwd)"
MODE="direct"
[ "${1:-}" = "--gateway" ] && MODE="gateway"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "mode=$MODE rounds=$ROUNDS"
for i in $(seq 1 "$ROUNDS"); do
  text="StreamKit observability sample, round $i: the quick brown fox."
  if [ "$MODE" = "gateway" ]; then
    curl -fsS -o "$tmp/a.ogg" -d "$text" "$GATEWAY_URL/tts"
    curl -fsS -o /dev/null --data-binary @"$tmp/a.ogg" -H 'Content-Type: audio/ogg' "$GATEWAY_URL/stt" || true
  else
    printf '%s' "$text" > "$tmp/in.txt"
    # X-StreamKit-Service lets a service-label-aware skit (see PR #545) split
    # oneshot metrics by {tts,stt}; older builds simply ignore the header.
    curl -fsS -o "$tmp/a.ogg" \
      -H 'X-StreamKit-Service: tts' \
      -F "config=<$HERE/pipelines/tts-kokoro.yml" \
      -F "media=@$tmp/in.txt;type=text/plain;filename=media" \
      "$SKIT_URL/api/v1/process"
    curl -fsS -o /dev/null \
      -H 'X-StreamKit-Service: stt' \
      -F "config=<$HERE/pipelines/stt-whisper.yml" \
      -F "media=@$tmp/a.ogg;type=audio/ogg;filename=media" \
      "$SKIT_URL/api/v1/process" || true
  fi
  printf '.'
done
echo " done"
