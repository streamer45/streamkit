#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# Drives TTS + STT traffic so the dashboards have data. By default it calls
# skit's oneshot /api/v1/process directly (no gateway required). Pass --gateway
# to route through the speech gateway instead (requires the gateway overlay:
# `docker compose -f docker-compose.yml -f docker-compose.gateway.yml up -d`),
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

# `docker compose up -d` returns before skit/the gateway are actually serving,
# so wait for the target to respond before the loop — otherwise the first
# request hits a still-starting service, fails, and `set -e` aborts with empty
# dashboards (the exact outcome this sample exists to prevent).
wait_for() {
  url="$1"; name="$2"
  for _ in $(seq 1 60); do
    curl -fsS -o /dev/null "$url" 2>/dev/null && return 0
    sleep 2
  done
  echo "error: $name not ready at $url after 120s" >&2
  exit 1
}

# Warn once (not per round) when an STT request comes back non-2xx, so a
# consistently-failing STT is visible without spamming the progress dots.
warn_stt_once() {
  case "$1" in
    2*) ;;
    *) [ -n "${stt_warned:-}" ] || { printf '\nnote: %s STT -> HTTP %s; the Whisper model the pipeline targets may not ship in the -demo image (see README / #553). TTS still populates its row.\n' "$2" "$1"; stt_warned=1; } ;;
  esac
}

if [ "$MODE" = "gateway" ]; then
  wait_for "$GATEWAY_URL/metrics" "speech gateway"
else
  wait_for "$SKIT_URL/healthz" "skit"
fi

echo "mode=$MODE rounds=$ROUNDS"
for i in $(seq 1 "$ROUNDS"); do
  text="StreamKit observability sample, round $i: the quick brown fox."
  if [ "$MODE" = "gateway" ]; then
    curl --retry 5 --retry-connrefused --retry-delay 1 -fsS -o "$tmp/a.ogg" -d "$text" "$GATEWAY_URL/tts"
    code=$(curl -s -o /dev/null -w '%{http_code}' --data-binary @"$tmp/a.ogg" -H 'Content-Type: audio/ogg' "$GATEWAY_URL/stt" || true)
    warn_stt_once "$code" gateway
  else
    printf '%s' "$text" > "$tmp/in.txt"
    # The pipelines declare `attributes: { service: tts|stt }`, which a
    # service-label-aware skit (see README / observability guide) turns into a
    # bounded `service` metric label; older builds simply ignore the field.
    curl --retry 5 --retry-connrefused --retry-delay 1 -fsS -o "$tmp/a.ogg" \
      -F "config=<$HERE/pipelines/tts-kokoro.yml" \
      -F "media=@$tmp/in.txt;type=text/plain;filename=media" \
      "$SKIT_URL/api/v1/process"
    code=$(curl -s -o /dev/null -w '%{http_code}' \
      -F "config=<$HERE/pipelines/stt-whisper.yml" \
      -F "media=@$tmp/a.ogg;type=audio/ogg;filename=media" \
      "$SKIT_URL/api/v1/process" || true)
    warn_stt_once "$code" direct
  fi
  printf '.'
done
echo " done"
