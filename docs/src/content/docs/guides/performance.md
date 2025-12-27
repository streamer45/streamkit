---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Performance Tuning
description: How to reason about buffering, batching, and latency in StreamKit
---

StreamKit performance tuning is mostly about two tradeoffs:

- **Latency vs buffering**: bigger queues smooth out spikes, but add delay.
- **Responsiveness vs throughput**: bigger batches reduce per-packet overhead, but make control messages (and backpressure) less immediate.

## Units

Most knobs fall into one of these categories:

- **`*_capacity`**: bounded channel depth in **packets** (engine data plane).
- **`buffer_size`**: bounded queue depth (usually **packets** or **frames**, node-specific).
- **`chunk_size` / `*_buffer_size`**: buffering threshold in **bytes** (I/O/container nodes).
- **`*_ms`**: timeouts in **milliseconds**.

## Dynamic Sessions (Engine)

Dynamic sessions (created via the sessions API) use bounded channels throughout the data plane. The main knobs live under `[engine]` in `skit.toml`.

### `profile`

Use a preset as a starting point:

```toml
[engine]
profile = "low-latency" # low-latency | balanced | high-throughput
```

Explicit capacities override the preset:

```toml
[engine]
profile = "low-latency"
node_input_capacity = 16
```

`profile` only provides defaults for channel capacities (it does not change node code or thread scheduling). If you set explicit values, those win.

### `node_input_capacity`

Per-input-pin buffer size (in packets). Higher values increase buffering and worst-case latency before upstream backpressure kicks in.

If your packets are 20ms audio frames, a capacity of `N` implies up to ~`N * 20ms` of queued audio **per input pin**, before additional downstream buffering.

### `pin_distributor_capacity`

Buffer between a node output and its per-pin distributor (fan-out/router).

This adds another bounded queue in the hot path, so worst-case queued packets per hop can be roughly:

`pin_distributor_capacity + node_input_capacity`

(plus any node-internal buffering).

### `packet_batch_size`

Controls how aggressively nodes batch-drain their input channels (via `NodeContext.batch_size`).

This is still used in production nodes (e.g. Opus decode/encode, gain, MoQ transport) and directly affects latency vs throughput.

## Oneshot HTTP Pipelines

Oneshot pipelines (HTTP batch processing via `/api/v1/process`) have their own configuration under `[engine.oneshot]`:

```toml
[engine.oneshot]
packet_batch_size = 32      # Same as dynamic, affects throughput
media_channel_capacity = 256  # Larger than dynamic for batch efficiency
io_channel_capacity = 16      # HTTP input/output stream buffers
```

| Option | Default | Description |
|--------|---------|-------------|
| `packet_batch_size` | 32 | Packets processed before yielding to control |
| `media_channel_capacity` | 256 | Buffer size between nodes (packets) |
| `io_channel_capacity` | 16 | HTTP I/O stream buffer (packets) |

Oneshot pipelines use larger defaults than dynamic sessions because they don't need tight backpressure coordination—they're optimized for throughput in batch scenarios.

## Node-Level Knobs (YAML params)

Many nodes expose their own buffering/throughput controls in pipeline YAML, for example:

- `core::pacer` / `audio::pacer`: `buffer_size` (queue depth)
- `containers::ogg::muxer` / `containers::webm::muxer`: `chunk_size` (flush threshold)
- `core::file_reader` / `core::file_writer` / `transport::http::fetcher`: `chunk_size`
- `audio::mixer`: `sync_timeout_ms` and (in clocked mode) `jitter_buffer_frames`
- `audio::resampler`: `chunk_frames` and `output_frame_size`

These are separate from engine channel capacities and can dominate end-to-end latency depending on the node.

## Advanced Internal Buffers

For power users who need to tune internal codec/container node buffers, the `[engine.advanced]` section exposes settings that were previously fixed:

```toml
[engine.advanced]
codec_channel_capacity = 32      # Opus async/blocking handoff
stream_channel_capacity = 8      # Bytes->blocking streaming handoff (demux/decoders)
demuxer_buffer_size = 65536      # OGG demuxer duplex buffer (bytes)
moq_peer_channel_capacity = 100  # (MoQ builds) MoQ transport internal queues (packets)
```

| Option | Default | Description |
|--------|---------|-------------|
| `codec_channel_capacity` | 32 | Async/blocking handoff in codec nodes |
| `stream_channel_capacity` | 8 | Streaming reader channel capacity (packets) |
| `demuxer_buffer_size` | 65536 | OGG demuxer duplex buffer (bytes) |
| `moq_peer_channel_capacity` | 100 | (MoQ builds) MoQ peer internal channels (packets) |

**Warning**: Only modify these if you understand the latency/throughput implications. The defaults are tuned for typical real-time audio processing workloads.

### When to Adjust

- **Increase `codec_channel_capacity`**: If you see backpressure warnings from codec nodes during high-throughput batch processing
- **Decrease `codec_channel_capacity`**: If you need ultra-low latency and can tolerate more CPU overhead
- **Increase `stream_channel_capacity`**: If streaming demux/decode tasks can't keep up with bursty inputs
- **Increase `demuxer_buffer_size`**: For large OGG files with complex structures
- **Increase `moq_peer_channel_capacity`**: If MoQ connections drop/lag during bursty send/receive periods (at the cost of memory/latency)

## Other Internal Queues (Fixed)

These exist for robustness but are not currently exposed as config:

- Dynamic engine control/query inboxes (graph ops + queries)
- Dynamic engine state/stats subscriber channels (websocket/UI watchers)

## Frame Pool (Fixed)

The core audio frame pool is preallocated with fixed defaults and cannot be configured at runtime:

- `crates/core/src/frame_pool.rs`: `DEFAULT_AUDIO_BUCKET_SIZES` = [960, 1920, 3840, 7680] samples
- `DEFAULT_AUDIO_BUFFERS_PER_BUCKET` = 32 buffers

These are optimized for common audio frame sizes (10-80ms at 48kHz) and should not need adjustment.

## Complete Example

```toml
# skit.toml - Performance tuning example

[engine]
profile = "balanced"
packet_batch_size = 32
node_input_capacity = 32
pin_distributor_capacity = 16

[engine.oneshot]
packet_batch_size = 32
media_channel_capacity = 256
io_channel_capacity = 16

[engine.advanced]
codec_channel_capacity = 32
stream_channel_capacity = 8
demuxer_buffer_size = 65536
moq_peer_channel_capacity = 100
```
