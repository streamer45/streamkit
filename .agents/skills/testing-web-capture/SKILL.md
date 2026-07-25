---
name: testing-web-capture
description: Testing the examples/web-capture gateway demo (servo plugin clip/cast endpoints) end-to-end against a local skit backend. Use when testing the servo native plugin, the web-capture Go gateway, or cross-session rendering behaviour.
---

# Testing the web-capture gateway demo

## Setup
1. Build and install the servo plugin: `just build-plugin-native-servo && just copy-plugins-native` (installs `.plugins/native/servo/libservo_web.so` + plugin.yml). The plugin is only loaded at skit startup — restart skit after swapping the `.so`.
2. Start the backend: `just skit` (127.0.0.1:4545). Auth mode defaults to Auto → disabled on loopback, so the gateway needs NO token locally.
3. Gateway (needs Go 1.24+; install from go.dev tarball if `go` is missing):
   `cd examples/web-capture && go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545`
4. Endpoints: `http://127.0.0.1:8080/clip/dur=5s,res=1280x720/example.com` (finite MP4), `http://127.0.0.1:8080/cast/<url>` (live WebM player page). SSRF guard blocks loopback/private targets — use public pages.

## Verification tips
- Don't trust "the video plays" — extract frames and check pixels:
  `ffmpeg -i clip.mp4 -vf "select='eq(n,60)'" -vsync 0 f.png` then check min/max grey levels with PIL. Servo may render blank white (background only, no text) on headless boxes with llvmpipe — this has been observed on both servo 0.1 and 0.4 pins, i.e. possibly environmental; compare against a baseline build before attributing to a PR.
- Cross-session leak regression test (heavy, llvmpipe):
  `cd plugins/native/servo && CARGO_TARGET_DIR=../../../target/plugins cargo test --release --test cross_session_leak`
  It uses `data:` URLs with solid red/blue backgrounds, so it works even when font/text rendering is broken. Under servo 0.4.0 the `data:` pages never reached LoadStatus::Complete/paint (test fails "never painted its own page"); under the 0.1 pin it passes.
- To A/B two plugin versions, use `git worktree` for the baseline; note `just build-plugin-native-servo` writes to the worktree's own `target/plugins`, not the main repo's.
- Cast autoplay may not start in Chrome for Testing; click the player's play button once.
- If the Devin browser tool is down, the real Chrome binary lives under `/opt/.devin/chrome/chrome/*/chrome-linux64/chrome`; drive it with `DISPLAY=:0`, `xdotool`, and `scrot` (`~/.local/bin/google-chrome` is only a proxy wrapper to the browser service).
