<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Pipeline validation

Headless tests that POST pipelines to a running `skit` server
(`/api/v1/process`) and validate the output with `ffprobe`. Two suites share
the harness in `src/lib.rs`:

| Suite | What it covers | Expectations live in |
|-------|----------------|----------------------|
| `tests/validate.rs` | hand-written fixtures under `samples/pipelines/test/<name>/` | a sibling `expected.toml` |
| `tests/oneshot.rs` | the official samples under `samples/pipelines/oneshot/*.yml` | one `[<stem>]` table in `oneshot-samples.toml` |

This crate is **excluded from the workspace**, so `just lint` / `just test`
don't touch it. Correctness is enforced by the `E2E / Pipeline Validation`
jobs. Run it locally against a server:

```sh
just test-pipelines        http://localhost:4545         # fixtures
just test-oneshot-samples  http://localhost:4545         # official samples
just test-oneshot-samples  http://localhost:4545 colorbars   # filter by name
```

## Adding an oneshot sample

`tests/oneshot.rs` discovers every `samples/pipelines/oneshot/*.yml` and
**fails** if one has no `[<stem>]` table — a new sample is never silently
uncovered. So when you add a sample, add its manifest entry too.

A media entry declares the [`MediaExpectations`] ffprobe checks plus skip
controls:

```toml
[my_new_sample]              # must match the YAML filename stem
output_extension = ".webm"
container_format = "matroska,webm"
codec_name = "vp9"
width = 1280
height = 720
```

For samples that emit JSON/NDJSON (transcription, VAD) set
`output_kind = "json"` and list substrings in `json_contains` instead of the
ffprobe fields.

`MediaExpectations` (`src/lib.rs`) is the **single source of truth** for the
ffprobe checks — it is flattened into both `Expected` and `OneshotEntry`, so a
new check (e.g. `bit_rate`) is added there once and both suites honour it.

## Skip controls

| Field | Effect |
|-------|--------|
| `requires_node` / `requires_nodes` | skip unless **all** listed node kinds are registered on the server |
| `optional_node = true` | skip a missing required node *even* under `PIPELINE_REQUIRE_NODES=1` — for nodes no CI job compiles (marketplace plugins, VA-API) |
| `requires_env = [...]` | skip unless every listed env var is set (e.g. S3 credentials) |
| `slow = true` | skip unless `PIPELINE_INCLUDE_SLOW=1` |

`PIPELINE_REQUIRE_NODES=1` (the GPU job) turns a missing **non-optional** node
into a failure rather than a skip, so a HW codec the GPU runner builds is caught
if it regresses. Note `slow` is checked *before* the node check: because
`video_nv_av1_colorbars` is `slow`, this suite only guards `vulkan_video` —
`nvcodec`/NVENC-AV1 registration is guarded by the `nv_av1_colorbars` fixture in
`tests/validate.rs` instead.

### When to mark a sample `slow`

Showcase pipelines that take many seconds of wall-clock — anything ending in a
real-time `core::pacer`, or a slow software AV1 encode — should be `slow` so PR
CI stays fast. They still run in the nightly
`pipeline-validation-nightly.yml` workflow (`PIPELINE_INCLUDE_SLOW=1`) and on
demand. Prefer relying on a cheap `samples/pipelines/test/` fixture for the raw
codec coverage and gating the heavy official sample behind `slow`.

[`MediaExpectations`]: src/lib.rs
