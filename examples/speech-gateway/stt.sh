#!/usr/bin/env bash
set -euo pipefail

# ---------------- config ----------------
STT_URL="${STT_URL:-https://stt.streamkit.dev}"
WORKDIR="$(mktemp -d /tmp/stt-XXXX)"
OGG="${WORKDIR}/capture.ogg"

# ---------------- platform detection ----------------
OS="$(uname -s)"
case "$OS" in
Linux)
  AUDIO_FMT="pulse"
  INPUT_DEV="default"
  ;;
Darwin)
  AUDIO_FMT="avfoundation"
  INPUT_DEV=":0"
  ;;
MINGW* | MSYS* | CYGWIN*)
  AUDIO_FMT="dshow"
  INPUT_DEV="audio=default"
  ;;
*)
  echo "Unsupported OS: ${OS}" >&2
  exit 1
  ;;
esac

# Allow override
AUDIO_FMT="${AUDIO_FMT_OVERRIDE:-$AUDIO_FMT}"
INPUT_DEV="${INPUT_DEV_OVERRIDE:-$INPUT_DEV}"

# ---------------- info ----------------
echo "OS:            ${OS}"
echo "Audio backend: ${AUDIO_FMT}"
echo "Input device:  ${INPUT_DEV}"
echo "Output file:   ${OGG}"
echo
echo "Initializing microphone…"

# ---------------- start capture ----------------
# Build ffmpeg args based on audio format
FFMPEG_ARGS=(-f "${AUDIO_FMT}")

# For PulseAudio, reduce buffer sizes to minimize latency
if [ "${AUDIO_FMT}" = "pulse" ]; then
  FFMPEG_ARGS+=(-fragment_size 1024)
fi

FFMPEG_ARGS+=(
  -i "${INPUT_DEV}"
  -ac 1 -ar 48000
  -af "volume=2.0"
  -flush_packets 1
  -c:a libopus
  -frame_duration 20
  -application voip
  -f ogg
  -hide_banner -loglevel info
  "${OGG}"
)

# Create a named pipe for stdin control
FFMPEG_PIPE="${WORKDIR}/ffmpeg.pipe"
mkfifo "${FFMPEG_PIPE}"

# Start ffmpeg with stdin from the pipe, keep it open in background
exec 3<>"${FFMPEG_PIPE}"
ffmpeg "${FFMPEG_ARGS[@]}" <"${FFMPEG_PIPE}" &
FFPID=$!

# Give ffmpeg a brief moment to initialize
sleep 0.1

echo
printf "\a"
echo "🎙️  Recording — speak now"

# ---------------- cleanup ----------------
cleanup() {
  echo
  echo "Stopping capture…"

  # Send 'q' to ffmpeg's stdin for graceful shutdown
  echo "q" >&3 2>/dev/null || true

  # Wait for ffmpeg to finish writing the file
  wait "${FFPID}" 2>/dev/null || true

  # Close the pipe
  exec 3>&- 2>/dev/null || true

  echo "Sending to STT…"
  curl --http1.1 --no-buffer -sS \
    -H "Content-Type: audio/ogg" \
    --data-binary @"${OGG}" \
    "${STT_URL}"
  echo

  echo "Capture kept at:"
  echo "  ${OGG}"
}

trap cleanup INT TERM
wait "${FFPID}"
