<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit — Priority Recommendations for Immediate User Value

*Analysis date: 2025-04-05*
*Author: Devin (code review & codebase analysis)*

---

## Context

StreamKit has made impressive architectural progress. In the last ~40 PRs alone,
the project has landed: GPU-accelerated compositing (wgpu), AV1 codec support
(SVT-AV1 encoder + dav1d decoder), MSE browser playback, screen sharing
foundation, schema-driven UI controls, upstream resize hints, a Slint UI plugin,
plugin SDK video support, and extensive performance/render profiling
infrastructure.

This is serious foundational work. But it's also almost entirely
**infrastructure-facing** — the kind of work that enables future capabilities
rather than the kind that a user trying StreamKit today would immediately feel.

Below is an honest assessment of what I think would deliver the most *tangible,
immediate* value to people who download StreamKit and try to use it for real
work. The recommendations are ordered by impact-to-effort ratio, not by
technical difficulty.

---

## TL;DR — Top 5 Priorities

| # | Priority | Why it matters now |
|---|----------|--------------------|
| 1 | **Golden-path "Screen + Cam PiP" end-to-end demo** | The canonical use case is *almost* there but has no polished, tested walkthrough. This is the single biggest "wow" moment for new users. |
| 2 | **Voice agent conversation memory (generic sample)** | The generic voice agent sample is stateless (the weather agent already has memory). Bringing the generic agent up to parity would make it dramatically more compelling as a starting point. |
| 3 | **Streamlined first-run experience (fewer prerequisites)** | Getting from `docker run` to a working audio pipeline is smooth, but the interesting demos (voice agent, video compositor) require multiple model downloads, `skit.toml` edits, and env vars. A `--demo` flag or guided setup would collapse this. |
| 4 | **Error message quality for common mistakes** | Pipeline validation errors are often terse Rust-internal messages. The most common user mistakes (wrong model path, missing plugin, pin type mismatch) deserve human-friendly, actionable error messages. |
| 5 | **WebSocket transport nodes for non-media data** | On the roadmap but not landed — this would unlock a huge class of integrations (webhooks, chat, events, dashboard data) that don't need MoQ's complexity. |

---

## Detailed Analysis

### 1. Golden-Path "Screen Share + Camera PiP" Demo

**Status:** The pipeline YAML exists (`video_moq_screen_cam_pip.yml`), the
compositor is capable, and the client section is wired up. But there is no
walkthrough, no README, no tutorial.

**Why it matters:** This is the use case that makes people say "I want to use
this." A polished screen-share-with-webcam-overlay demo, with clear steps and a
60-second video, would be the single best marketing asset for the project. It
demonstrates the compositor, MoQ transport, video codecs, and the Stream View UI
all at once.

**What's needed:**
- A `SCREEN_SHARE_PIP.md` companion doc (like `VOICE_AGENT.md`) with step-by-step
  instructions, screenshots, and troubleshooting.
- Verify the pipeline works end-to-end with the current Stream View UI (screen
  capture + camera permissions, codec negotiation, etc.).
- A short recording or GIF showing the result in action.
- Consider making this the *default* sample that loads when you open Stream View
  for the first time.

**Effort:** Low-medium (mostly testing + documentation, not new code).

---

### 2. Voice Agent Conversation Memory (Generic Sample)

**Status:** The weather agent (`voice-weather-open-meteo.js`) already has proper
conversation memory — a sliding window of 12 messages (6 turns) plus location
persistence via `lastResolved`. However, the *generic* voice agent
(`voice-agent-openai.yaml`) is fully stateless: its inline script sends only
`[system, user]` per request with no history. The `VOICE_AGENT.md` doc even
calls this out under "Adding Conversation History" and punts to Redis/database.

**Why it matters:** The generic voice agent is the more prominent sample and
likely the first one new users try. A voice agent that can't remember what you
said 10 seconds ago feels like a toy. The weather agent already proves the
pattern works — the generic agent just needs to be brought up to parity.

**What's needed:**
- Update the `openai_processor` script node in `voice-agent-openai.yaml` to
  maintain a `messages` array across `process()` calls, following the same
  pattern used in `voice-weather-open-meteo.js` (`pushConversation()` +
  `MAX_CONVERSATION_MESSAGES` sliding window).
- Update `VOICE_AGENT.md` to document the conversation memory behavior and
  remove/update the section that punts to Redis/database.

**Effort:** Very low — this is a ~20-line change to the inline JavaScript in the
YAML sample, plus doc updates. The pattern is already proven in the weather
agent.

---

### 3. Streamlined First-Run Experience

**Status:** The Docker quickstart is clean for the basic `double_volume.yml`
audio gain demo. But the demos that actually show StreamKit's unique value
(voice agents, video compositing) require:

1. Downloading specific models (Whisper, VAD, Kokoro) — multiple steps, ~400 MB.
2. Editing `skit.toml` to add fetch allowlists and secrets.
3. Setting environment variables (`OPENAI_API_KEY`).
4. Understanding MoQ transport concepts.

This is too many steps between "I'm curious" and "wow."

**What's needed (pick any subset):**
- **`skit serve --demo` mode**: Auto-enable a pre-configured `skit.toml` with
  common allowlists (OpenAI, Open-Meteo) and secrets (read from env). The demo
  Docker image already bundles models — this flag would remove the `skit.toml`
  friction.
- **Interactive first-run wizard in the UI**: When no sessions exist and no
  plugins are loaded, show a guided "Get Started" card in the Monitor View that
  walks through loading a sample pipeline and connecting.
- **`just demo` recipe**: A single command that downloads models, generates a
  demo `skit.toml`, and starts the server. The existing `just download-models`
  is a good start but doesn't wire up the config.
- **Pre-configured demo `skit.toml`**: Ship a `skit.demo.toml` in the repo that
  has the allowlists/secrets for all sample pipelines, with clear comments.

**Effort:** Medium (the `--demo` flag and demo config are low-effort; the UI
wizard is higher).

---

### 4. Error Message Quality

**Status:** Pipeline validation catches real errors (type mismatches, missing
nodes, invalid pins), but the messages are often raw Rust error strings that
don't help the user fix the problem.

**Examples of common mistakes that deserve better messages:**
- **Wrong model path** → Currently surfaces as a plugin init failure with an
  opaque error. Should say: "Model file not found at `models/ggml-tiny.en-q5_1.bin`.
  Run `just download-whisper-models` or check the path."
- **Missing plugin** → "Unknown node kind `plugin::native::whisper`" doesn't
  tell you to install the plugin. Should link to the marketplace or `just
  build-whisper`.
- **Pin type mismatch** → The type system catches this, but the error doesn't
  say *what types* were expected vs. provided. "Cannot connect audio::gain.out
  to video::compositor.in_0: expected VideoFrame, got AudioFrame" would be
  immediately actionable.
- **Missing `skit.toml` config for script fetch** → The "Fetch blocked" error
  exists and is decent, but could include the exact TOML snippet needed.

**What's needed:**
- Audit the top ~10 error paths in pipeline validation and session creation.
- For each, add context: what happened, why it's wrong, and how to fix it.
- Consider a `--validate` CLI command that checks a pipeline YAML without
  running it, and reports all issues at once (not just the first).

**Effort:** Medium (requires touching several crates, but each individual error
improvement is small).

---

### 5. WebSocket Transport Nodes (Non-Media)

**Status:** On the roadmap ("near-term"), not yet implemented. Currently all
real-time data flow requires MoQ/WebTransport, which is powerful but complex.

**Why it matters:** Many real-world integrations don't involve media at all:
- Sending transcription results to a dashboard
- Receiving chat messages to feed into TTS
- Webhook-style event triggers (start/stop recording, switch scenes)
- Feeding subtitle text to an overlay
- Control plane integration with external systems

The `core::script` node can `fetch()` external APIs, but that's
request/response — not streaming. A WebSocket subscriber/publisher node pair
would let StreamKit pipelines interact with the outside world in real time
without requiring MoQ clients.

**What's needed:**
- `transport::ws::subscriber` — connects to an external WebSocket URL, emits
  received messages as Text/Custom packets.
- `transport::ws::publisher` — accepts packets and sends them to connected
  WebSocket clients (or to an external WebSocket URL).
- Consider a `transport::ws::peer` (bidirectional) for the common case.

**Effort:** Medium-high (new node implementations + connection management), but
the architectural patterns already exist in the MoQ transport nodes.

---

## Honorable Mentions (High Value, Lower Urgency)

### A/V Sync Polish

The roadmap flags this as P0, and it's genuinely important for production use.
The MSE player landed recently (#222) with sync fixes, but there's no
regression test suite for drift/jitter scenarios. This is the kind of thing
that's invisible when it works and devastating when it doesn't.

### RTMP Input Node

An RTMP ingest node would immediately connect StreamKit to the existing
streaming ecosystem (OBS, hardware encoders, etc.). This is on the roadmap and
would unlock a large class of users who already have RTMP-based workflows.

### S3 Sink Node

Also on the roadmap. "Process audio/video and write the result to S3" is a
bread-and-butter use case for batch/oneshot pipelines, and currently requires
external scripting.

### Pipeline Templates / Wizards in the UI

The Design View is powerful but assumes you know what nodes exist and how to
wire them. A "Create from Template" flow that offers common patterns (STT
pipeline, TTS pipeline, voice agent, video compositor) with pre-filled YAML
and guided parameter entry would dramatically lower the barrier to entry.

### `skit-cli` Improvements

The CLI exists but isn't prominently documented or demoed. For CI/CD and
scripting use cases, having a polished CLI experience (with good `--help`,
JSON output, and bash completion) would make StreamKit much more attractive
for automation.

---

## What I'd Recommend Tackling First (Effort vs. Impact)

```
Impact ▲
       │
  HIGH │  ② Voice Memory    ① Screen PiP Demo
       │  (very low effort)  (low-med effort)
       │
       │  ③ First-Run UX    ④ Error Messages
  MED  │  (medium effort)    (medium effort)
       │
       │                     ⑤ WS Transport
  LOW  │                     (med-high effort)
       │
       └──────────────────────────────────────► Effort
          LOW              MEDIUM           HIGH
```

**If I had one day:** Do #2 (voice memory) — it's the highest-value change for
the lowest effort.

**If I had one week:** Do #1 (golden-path demo) + #2 (voice memory) + start #3
(demo config).

**If I had one sprint:** All five, with #4 (error messages) being an ongoing
effort that improves incrementally.

---

## Closing Thought

The foundation you've built is genuinely impressive — the engine architecture,
the type system, the compositor, the plugin SDK, MoQ transport, the perf
infrastructure. It's the kind of work that compounds over time.

But right now, the gap between "what StreamKit can do" and "what a new user
experiences in their first 30 minutes" is wider than it needs to be. The
recommendations above are mostly about bridging that gap — making the existing
capabilities *accessible* rather than building new ones.

The architectural work shouldn't stop (A/V sync, timing contracts, API
stabilization are all important). But interspersing it with user-facing
polish would keep the project grounded and help attract early adopters who
can give you real feedback on what to build next.
