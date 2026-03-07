// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video compositor node.
//!
//! Composites multiple raw video inputs onto a single RGBA8 output canvas with
//! optional image and text overlays. Supports dynamic pin creation for
//! attaching arbitrary inputs at runtime.
//!
//! - Inputs accept `RawVideo(RGBA8)` with wildcard dimensions.
//! - Output produces `RawVideo(RGBA8)` at the configured canvas size.
//! - Heavy compositing work runs on a persistent blocking thread (via
//!   `spawn_blocking`) to avoid blocking the async runtime and to keep CPU
//!   caches warm across frames.
//! - Row-level parallelism via `rayon` for blitting and pixel-format
//!   conversion.
//! - Image overlays are decoded once during initialization (PNG/JPEG via the
//!   `image` crate).
//! - Text overlays are rasterized via `fontdue` once per `UpdateParams`, not
//!   per frame.
//!
//! # Future work
//! - GPU-accelerated compositing via `wgpu`.
//! - Bilinear / Lanczos scaling (MVP uses nearest-neighbor).

pub mod config;
pub mod kernel;
pub mod overlay;
pub mod pixel_ops;

use async_trait::async_trait;
use config::CompositorConfig;
use kernel::{CompositeResult, CompositeWorkItem, LayerSnapshot};
use opentelemetry::{global, KeyValue};
use overlay::{decode_image_overlay, rasterize_text_overlay, DecodedOverlay};
use schemars::schema_for;
use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::registry::StaticPins;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    Packet, PacketMetadata, PacketType, PixelFormat, VideoFormat, VideoFrame,
};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

use kernel::{composite_frame, ConversionCache};

// ── Input slot ──────────────────────────────────────────────────────────────

/// Holds a receiver and the most-recently-received frame for one input layer.
struct InputSlot {
    name: String,
    rx: mpsc::Receiver<Packet>,
    latest_frame: Option<VideoFrame>,
}

// ── Cached layer config ─────────────────────────────────────────────────────

/// Pre-resolved layer configuration for a single slot.
/// Rebuilt only when compositor config or pin set changes, avoiding
/// per-frame `HashMap` lookups and `sort_by` calls.
#[derive(Clone)]
struct ResolvedSlotConfig {
    rect: Option<config::Rect>,
    opacity: f32,
    z_index: i32,
    rotation_degrees: f32,
    /// When `true`, the source is fitted within the destination rect
    /// while preserving its aspect ratio (letterbox / pillarbox).
    /// Used by auto-PiP layers to avoid stretching.
    aspect_fit: bool,
}

/// Rebuild the per-slot resolved configs and the z-sorted draw order.
///
/// Called once at startup and whenever `UpdateParams` or pin management
/// changes the layer set.  The returned draw order is a list of slot
/// indices sorted by `(z_index, slot_index)`.
fn rebuild_layer_cache(
    slots: &[InputSlot],
    config: &CompositorConfig,
) -> (Vec<ResolvedSlotConfig>, Vec<usize>) {
    let num_slots = slots.len();
    let mut configs: Vec<ResolvedSlotConfig> = Vec::with_capacity(num_slots);
    for (idx, slot) in slots.iter().enumerate() {
        let layer_cfg = config.layers.get(&slot.name);
        #[allow(clippy::option_if_let_else)]
        let (rect, opacity, z_index, rotation_degrees, aspect_fit) = if let Some(lc) = layer_cfg {
            (lc.rect.clone(), lc.opacity, lc.z_index, lc.rotation_degrees, false)
        } else if idx > 0 && num_slots > 1 {
            // Auto-PiP: non-first layers without explicit config.
            let pip_w = config.width / 3;
            let pip_h = config.height / 3;
            #[allow(clippy::cast_possible_wrap)]
            let pip_x = (config.width - pip_w - 20) as i32;
            #[allow(clippy::cast_possible_wrap)]
            let pip_y = (config.height - pip_h - 20) as i32;
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            (
                Some(config::Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                1.0,
                idx as i32,
                0.0,
                true, // preserve source aspect ratio within PiP bounds
            )
        } else {
            (None, 1.0, 0, 0.0, false)
        };
        configs.push(ResolvedSlotConfig { rect, opacity, z_index, rotation_degrees, aspect_fit });
    }

    // Pre-sort by (z_index, slot_index).
    let mut draw_order: Vec<usize> = (0..num_slots).collect();
    draw_order.sort_by(|&a, &b| configs[a].z_index.cmp(&configs[b].z_index).then(a.cmp(&b)));

    (configs, draw_order)
}

/// Compute a destination rect that fits `src_w × src_h` within `bounds`
/// while preserving the source aspect ratio.  The fitted rect is centred
/// within the bounds.
fn fit_rect_preserving_aspect(src_w: u32, src_h: u32, bounds: &config::Rect) -> config::Rect {
    if src_w == 0 || src_h == 0 || bounds.width == 0 || bounds.height == 0 {
        return bounds.clone();
    }
    let scale_w = f64::from(bounds.width) / f64::from(src_w);
    let scale_h = f64::from(bounds.height) / f64::from(src_h);
    let scale = scale_w.min(scale_h);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fit_w = (f64::from(src_w) * scale).round() as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fit_h = (f64::from(src_h) * scale).round() as u32;
    // Centre within the bounding rect.
    #[allow(clippy::cast_possible_wrap)]
    let offset_x = (bounds.width.saturating_sub(fit_w) / 2) as i32;
    #[allow(clippy::cast_possible_wrap)]
    let offset_y = (bounds.height.saturating_sub(fit_h) / 2) as i32;
    config::Rect { x: bounds.x + offset_x, y: bounds.y + offset_y, width: fit_w, height: fit_h }
}

// ── Node ────────────────────────────────────────────────────────────────────

/// Composites multiple raw video inputs onto a single RGBA8 canvas with
/// optional image/text overlays.
///
/// Inputs are dynamic (`PinCardinality::Dynamic`) and can be attached at
/// runtime. Each input accepts `RawVideo(RGBA8)` or `RawVideo(I420)` with
/// wildcard dimensions.
///
/// Output `"out"` always produces `RawVideo(RGBA8)` at the configured canvas
/// size.  Downstream nodes (e.g. the VP9 encoder) are responsible for any
/// further format conversion.
pub struct CompositorNode {
    config: CompositorConfig,
    /// Current input pins (may grow dynamically).
    input_pins: Vec<InputPin>,
    /// Next input ID for dynamic pin naming.
    next_input_id: usize,
}

impl CompositorNode {
    #[must_use]
    pub fn new(config: CompositorConfig) -> Self {
        let (input_pins, next_input_id) = config.num_inputs.map_or_else(
            || {
                // Dynamic mode - start with no pins
                (Vec::new(), 0)
            },
            |num_inputs| {
                // Pre-create pins for stateless/oneshot pipelines.
                // Follow the YAML convention: single input uses "in",
                // multiple inputs use "in_0", "in_1", etc.
                let mut pins = Vec::with_capacity(num_inputs);
                if num_inputs == 1 {
                    pins.push(Self::make_input_pin("in".to_string()));
                } else {
                    for i in 0..num_inputs {
                        pins.push(Self::make_input_pin(format!("in_{i}")));
                    }
                }
                (pins, num_inputs)
            },
        );

        Self { config, input_pins, next_input_id }
    }

    /// The set of video packet types accepted by compositor input pins.
    fn accepted_video_types() -> Vec<PacketType> {
        vec![
            PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Rgba8,
            }),
            PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::I420,
            }),
            PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Nv12,
            }),
        ]
    }

    /// Returns the definition-time pins for registry (dynamic template).
    pub fn definition_pins() -> (Vec<InputPin>, Vec<OutputPin>) {
        let inputs = vec![InputPin {
            name: "in".to_string(),
            accepts_types: Self::accepted_video_types(),
            cardinality: PinCardinality::Dynamic { prefix: "in".to_string() },
        }];

        let outputs = vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Rgba8,
            }),
            cardinality: PinCardinality::Broadcast,
        }];

        (inputs, outputs)
    }

    /// Create a concrete `InputPin` for a given name.
    fn make_input_pin(name: String) -> InputPin {
        InputPin {
            name,
            accepts_types: Self::accepted_video_types(),
            cardinality: PinCardinality::One,
        }
    }
}

#[async_trait]
impl ProcessorNode for CompositorNode {
    fn input_pins(&self) -> Vec<InputPin> {
        self.input_pins.clone()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(VideoFormat {
                width: Some(self.config.width),
                height: Some(self.config.height),
                pixel_format: PixelFormat::Rgba8,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "CompositorNode starting: {}x{} canvas, {} image overlays, {} text overlays",
            self.config.width,
            self.config.height,
            self.config.image_overlays.len(),
            self.config.text_overlays.len(),
        );

        // Decode image overlays (once).  Wrap in Arc so per-frame clones
        // into the work item are cheap reference-count bumps.
        //
        // `image_overlay_cfg_indices` records, for each successfully decoded
        // overlay, the index of the originating `ImageOverlayConfig` in
        // `config.image_overlays`.  This allows the cache in
        // `apply_update_params` to map decoded bitmaps back to their configs
        // without relying on dimension-matching heuristics.
        let mut image_overlays_vec: Vec<Arc<DecodedOverlay>> =
            Vec::with_capacity(self.config.image_overlays.len());
        let mut image_overlay_cfg_indices: Vec<usize> =
            Vec::with_capacity(self.config.image_overlays.len());
        for (i, img_cfg) in self.config.image_overlays.iter().enumerate() {
            match decode_image_overlay(img_cfg) {
                Ok(overlay) => {
                    tracing::info!(
                        "Decoded image overlay {}: {}x{} -> rect ({},{} {}x{})",
                        i,
                        overlay.width,
                        overlay.height,
                        overlay.rect.x,
                        overlay.rect.y,
                        overlay.rect.width,
                        overlay.rect.height,
                    );
                    image_overlays_vec.push(Arc::new(overlay));
                    image_overlay_cfg_indices.push(i);
                },
                Err(e) => {
                    tracing::warn!("Failed to decode image overlay {}: {}", i, e);
                },
            }
        }

        // Rasterize text overlays (once; re-done on UpdateParams).  Also Arc-wrapped.
        let mut text_overlays_vec: Vec<Arc<DecodedOverlay>> =
            Vec::with_capacity(self.config.text_overlays.len());
        for txt_cfg in &self.config.text_overlays {
            text_overlays_vec.push(Arc::new(rasterize_text_overlay(txt_cfg)));
        }

        // Wrap in Arc<[...]> so per-frame clones into the work item are
        // a single ref-count bump instead of cloning the entire Vec.
        let mut image_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(image_overlays_vec);
        let mut text_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(text_overlays_vec);

        // Collect initial input slots from pre-connected pins.
        let mut slots: Vec<InputSlot> = Vec::new();
        for pin_name in context.inputs.keys() {
            let pin = Self::make_input_pin(pin_name.clone());
            self.input_pins.push(pin);
            // Track next_input_id for dynamically named pins.
            if let Some(num_str) = pin_name.strip_prefix("in_") {
                if let Ok(n) = num_str.parse::<usize>() {
                    self.next_input_id = self.next_input_id.max(n + 1);
                }
            }
        }
        // Drain all pre-connected inputs into slots.
        // IMPORTANT: HashMap::drain() has non-deterministic iteration order,
        // so we must sort by pin name to ensure stable slot ordering.
        // The slot index determines layer stacking (idx 0 = background,
        // idx > 0 = auto-PiP), so non-deterministic order would randomly
        // swap which input becomes the background vs. the PiP overlay.
        let mut pre_inputs: Vec<(String, mpsc::Receiver<Packet>)> =
            context.inputs.drain().collect();
        pre_inputs.sort_by(|(a, _), (b, _)| {
            // Sort numerically by the suffix of "in_N" pin names so that
            // in_0 < in_1 < ... < in_10.  Fall back to lexicographic order
            // for non-standard pin names.
            let a_num = a.strip_prefix("in_").and_then(|s| s.parse::<usize>().ok());
            let b_num = b.strip_prefix("in_").and_then(|s| s.parse::<usize>().ok());
            match (a_num, b_num) {
                (Some(an), Some(bn)) => an.cmp(&bn),
                _ => a.cmp(b),
            }
        });
        for (name, rx) in pre_inputs {
            tracing::info!("CompositorNode: pre-connected input '{}'", name);
            slots.push(InputSlot { name, rx, latest_frame: None });
        }

        // Pin management channel (optional).
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Shared state for the compositing thread.
        let video_pool = context.video_pool.clone();

        // ── Persistent compositing thread ───────────────────────────────
        // Instead of spawning a new blocking task per frame, we keep a
        // single long-lived thread that processes compositing work items
        // sent via a channel.  This avoids per-frame thread-pool
        // scheduling overhead and keeps CPU caches warm.
        let (work_tx, mut work_rx) = tokio::sync::mpsc::channel::<CompositeWorkItem>(2);
        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<CompositeResult>(2);

        let composite_thread = tokio::task::spawn_blocking(move || {
            // Per-slot cache for YUV→RGBA conversions. Avoids redundant
            // conversion when the source Arc hasn't changed between frames.
            let mut conversion_cache = ConversionCache::new();

            while let Some(work) = work_rx.blocking_recv() {
                let rgba_buf = composite_frame(
                    work.canvas_w,
                    work.canvas_h,
                    &work.layers,
                    &work.overlays,
                    work.video_pool.as_deref(),
                    &mut conversion_cache,
                );
                let result = CompositeResult { rgba_data: rgba_buf };
                if result_tx.blocking_send(result).is_err() {
                    break;
                }
            }
        });

        let mut output_seq: u64 = 0;
        let mut stop_reason: &str = "shutdown";

        // ── OpenTelemetry metrics ───────────────────────────────────────
        let meter = global::meter("skit_nodes");
        let frames_dropped_counter = meter
            .u64_counter("compositor.frames_dropped")
            .with_description("Frames dropped by the compositor to keep up with real-time input")
            .build();
        let otel_attrs = [KeyValue::new("node", node_name.clone())];

        // ── Fixed-rate tick ──────────────────────────────────────────────
        // The compositor runs at a fixed fps regardless of input rates,
        // like the audio clocked mixer.  On each tick it drains all inputs
        // to their latest frame and composites.  Inputs that haven't
        // delivered a new frame since the last tick reuse their previous
        // frame.  This guarantees a constant output rate and decouples
        // the compositor from input timing.
        let tick_duration =
            std::time::Duration::from_nanos(1_000_000_000u64 / u64::from(self.config.fps));
        let mut tick = tokio::time::interval(tick_duration);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // ── Cached layer config + draw order ────────────────────────────
        let mut layer_configs_dirty = true;
        let mut overlays_dirty = true;
        let mut resolved_configs: Vec<ResolvedSlotConfig> = Vec::new();
        let mut sorted_draw_order: Vec<usize> = Vec::new();
        let mut cached_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(Vec::new());

        loop {
            // ── Wait for the next tick, or handle control / pin msgs ────
            tokio::select! {
                biased;

                // Control messages (highest priority).
                Some(ctrl_msg) = context.control_rx.recv() => {
                    match ctrl_msg {
                        NodeControlMessage::Shutdown => {
                            tracing::info!("CompositorNode received shutdown");
                            break;
                        },
                        NodeControlMessage::UpdateParams(params) => {
                            let old_fps = self.config.fps;
                            Self::apply_update_params(
                                &mut self.config,
                                &mut image_overlays,
                                &mut image_overlay_cfg_indices,
                                &mut text_overlays,
                                params,
                                &mut stats_tracker,
                            );
                            layer_configs_dirty = true;
                            overlays_dirty = true;
                            if self.config.fps != old_fps {
                                let new_duration = std::time::Duration::from_nanos(
                                    1_000_000_000u64 / u64::from(self.config.fps),
                                );
                                tick = tokio::time::interval(new_duration);
                                tick.set_missed_tick_behavior(
                                    tokio::time::MissedTickBehavior::Skip,
                                );
                                tracing::info!("Compositor fps changed: {} → {}", old_fps, self.config.fps);
                            }
                        },
                        NodeControlMessage::Start => {},
                    }
                    continue;
                }

                // Pin management.
                Some(msg) = async {
                    match &mut pin_mgmt_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    Self::handle_pin_management(
                        &mut self,
                        msg,
                        &mut slots,
                    );
                    layer_configs_dirty = true;
                    continue;
                }

                // Fixed-rate tick — time to composite.
                _ = tick.tick() => {}
            }

            // ── Drain each slot to its latest frame (non-blocking) ──────
            for slot in &mut slots {
                let mut latest: Option<VideoFrame> = None;
                let mut dropped: u64 = 0;
                while let Ok(Packet::Video(frame)) = slot.rx.try_recv() {
                    if latest.is_some() {
                        dropped += 1;
                    }
                    latest = Some(frame);
                }
                if dropped > 0 {
                    frames_dropped_counter.add(dropped, &otel_attrs);
                    stats_tracker.discarded_n(dropped);
                }
                if let Some(frame) = latest {
                    slot.latest_frame = Some(frame);
                }
            }

            // Nothing to composite if no slot has ever received a frame.
            if !slots.iter().any(|s| s.latest_frame.is_some()) {
                continue;
            }

            // Check for closed input channels.
            let mut i = 0;
            while i < slots.len() {
                // A slot whose channel is closed AND has no buffered frame
                // can be removed.  We detect closure by a failed try_recv
                // returning Disconnected — but try_recv above already
                // drained.  Use a zero-capacity poll instead:
                if slots[i].rx.is_closed() {
                    tracing::info!("CompositorNode: input '{}' closed", slots[i].name);
                    slots.remove(i);
                    layer_configs_dirty = true;
                } else {
                    i += 1;
                }
            }
            if slots.is_empty() {
                stop_reason = "all_inputs_closed";
                break;
            }

            // ── Rebuild layer config cache if needed ─────────────────────
            if layer_configs_dirty {
                let (cfgs, order) = rebuild_layer_cache(&slots, &self.config);
                resolved_configs = cfgs;
                sorted_draw_order = order;
                layer_configs_dirty = false;
            }

            // ── Send work to persistent compositing thread ─────────────
            // Build layer snapshots in pre-sorted draw order using the
            // cached per-slot configs (no HashMap lookup, no sort).
            let layers: Vec<Option<LayerSnapshot>> = sorted_draw_order
                .iter()
                .map(|&idx| {
                    slots[idx].latest_frame.as_ref().map(|f| {
                        let cfg = &resolved_configs[idx];
                        let rect = if cfg.aspect_fit {
                            // Fit the source within the destination rect
                            // while preserving its aspect ratio.
                            cfg.rect
                                .as_ref()
                                .map(|r| fit_rect_preserving_aspect(f.width, f.height, r))
                        } else {
                            cfg.rect.clone()
                        };
                        LayerSnapshot {
                            data: f.data.clone(),
                            width: f.width,
                            height: f.height,
                            pixel_format: f.pixel_format,
                            rect,
                            opacity: cfg.opacity,
                            z_index: cfg.z_index,
                            rotation_degrees: cfg.rotation_degrees,
                        }
                    })
                })
                .collect();

            stats_tracker.received();

            // Rebuild merged overlay list only when overlays changed.
            if overlays_dirty {
                cached_overlays = Arc::from(
                    image_overlays.iter().chain(text_overlays.iter()).cloned().collect::<Vec<_>>(),
                );
                overlays_dirty = false;
            }

            // If everything is invisible (all layers + overlays at opacity 0),
            // skip compositing entirely.  This avoids the expensive RGBA→NV12
            // conversion downstream when there's nothing to draw.
            let any_visible_layer =
                layers.iter().any(|l| l.as_ref().is_some_and(|s| s.opacity > 0.0));
            let any_visible_overlay = cached_overlays.iter().any(|ov| ov.opacity > 0.0);
            if !any_visible_layer && !any_visible_overlay {
                continue;
            }

            let work_item = CompositeWorkItem {
                canvas_w: self.config.width,
                canvas_h: self.config.height,
                layers,
                overlays: cached_overlays.clone(),
                video_pool: video_pool.clone(),
            };

            // Send work to the compositing thread.  The work channel has
            // capacity 2, so at most one item can be in-flight while we
            // submit the next.  Use try_send to avoid blocking — if the
            // compositing thread hasn't finished the previous frame yet,
            // drop this one to stay real-time.
            match work_tx.try_send(work_item) {
                Ok(()) => {},
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    // Compositing thread is still busy — skip this frame.
                    frames_dropped_counter.add(1, &otel_attrs);
                    stats_tracker.discarded();
                    continue;
                },
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!("Compositing thread gone, stopping CompositorNode");
                    stop_reason = "compositor_thread_gone";
                    break;
                },
            }

            let Some(composite_result) = result_rx.recv().await else {
                tracing::debug!("Compositing result channel closed");
                stop_reason = "compositor_thread_gone";
                break;
            };

            // Build metadata from the first available input frame.
            let src_metadata =
                slots.iter().find_map(|s| s.latest_frame.as_ref()).and_then(|f| f.metadata.clone());

            let metadata = Some(PacketMetadata {
                timestamp_us: src_metadata.as_ref().and_then(|m| m.timestamp_us),
                duration_us: src_metadata.as_ref().and_then(|m| m.duration_us),
                sequence: Some(output_seq),
                // Don't set keyframe — the compositor outputs raw RGBA, not
                // encoded video.  Downstream encoders (VP9) decide their own
                // keyframe placement via kf_max_dist.  Setting this to true
                // caused every frame to be force-keyframed via VPX_EFLAG_FORCE_KF,
                // creating one MoQ group per frame and overwhelming the browser.
                keyframe: None,
            });

            let out_frame = VideoFrame::from_pooled(
                self.config.width,
                self.config.height,
                PixelFormat::Rgba8,
                composite_result.rgba_data,
                metadata,
            )?;

            // Non-blocking output send — if downstream (VP9 encoder) is
            // backed up, drop the frame rather than stalling the
            // compositor loop.  ChannelClosed is permanent (downstream
            // gone), so we stop the node.
            match context.output_sender.try_send("out", Packet::Video(out_frame)) {
                Ok(()) => {},
                Err(streamkit_core::node::OutputSendError::ChannelFull { .. }) => {
                    frames_dropped_counter.add(1, &otel_attrs);
                    stats_tracker.discarded();
                    output_seq += 1;
                    continue;
                },
                Err(_) => {
                    tracing::debug!("Output channel closed, stopping CompositorNode");
                    stop_reason = "output_closed";
                    break;
                },
            }

            stats_tracker.sent();
            stats_tracker.maybe_send();
            output_seq += 1;
        }

        // Drop the work sender to signal the compositing thread to exit.
        // NOTE: Any composite result currently in-flight (sent to the thread
        // but not yet received back via result_rx) will be lost here.  This is
        // acceptable for shutdown semantics — we prefer a fast exit over
        // draining one extra frame that may never be forwarded downstream.
        drop(work_tx);
        let _ = composite_thread.await;

        stats_tracker.force_send();
        state_helpers::emit_stopped(&context.state_tx, &node_name, stop_reason);
        Ok(())
    }
}

// ── Private helpers on CompositorNode ───────────────────────────────────────

impl CompositorNode {
    fn apply_update_params(
        config: &mut CompositorConfig,
        image_overlays: &mut Arc<[Arc<DecodedOverlay>]>,
        image_overlay_cfg_indices: &mut Vec<usize>,
        text_overlays: &mut Arc<[Arc<DecodedOverlay>]>,
        params: serde_json::Value,
        stats_tracker: &mut NodeStatsTracker,
    ) {
        match serde_json::from_value::<CompositorConfig>(params) {
            Ok(new_config) => match new_config.validate() {
                Ok(()) => {
                    tracing::info!(
                        old_w = config.width,
                        old_h = config.height,
                        new_w = new_config.width,
                        new_h = new_config.height,
                        "Updating compositor config"
                    );

                    // Re-decode image overlays only when their content or
                    // target rect changed.  When only video-layer positions
                    // are updated (the common case) the existing decoded
                    // bitmaps are reused via Arc, avoiding redundant base64
                    // decode + bilinear prescale work.
                    //
                    // The cache is keyed by (data_base64, width, height).
                    // `image_overlay_cfg_indices` provides an exact mapping
                    // from each decoded overlay back to its originating
                    // config index, eliminating any heuristic guessing
                    // about which decoded bitmap belongs to which config.
                    let old_imgs = image_overlays.clone();
                    let old_cfgs = &config.image_overlays;

                    let mut cache: HashMap<(&str, u32, u32), Vec<Arc<DecodedOverlay>>> =
                        HashMap::new();

                    // Each decoded overlay has a recorded config index in
                    // `image_overlay_cfg_indices`.  Use this to look up
                    // the originating config directly — no dimension
                    // matching needed.
                    for (dec_idx, decoded) in old_imgs.iter().enumerate() {
                        if let Some(&cfg_idx) = image_overlay_cfg_indices.get(dec_idx) {
                            if let Some(old_cfg) = old_cfgs.get(cfg_idx) {
                                let key = (
                                    old_cfg.data_base64.as_str(),
                                    old_cfg.transform.rect.width,
                                    old_cfg.transform.rect.height,
                                );
                                cache.entry(key).or_default().push(Arc::clone(decoded));
                            }
                        }
                    }

                    let mut new_image_overlays: Vec<Arc<DecodedOverlay>> =
                        Vec::with_capacity(new_config.image_overlays.len());
                    let mut new_cfg_indices: Vec<usize> =
                        Vec::with_capacity(new_config.image_overlays.len());
                    for (new_idx, img_cfg) in new_config.image_overlays.iter().enumerate() {
                        let key = (
                            img_cfg.data_base64.as_str(),
                            img_cfg.transform.rect.width,
                            img_cfg.transform.rect.height,
                        );
                        if let Some(entries) = cache.get_mut(&key) {
                            if let Some(existing) = entries.pop() {
                                // Content and target dimensions unchanged —
                                // reuse the decoded bitmap.  The overlay's
                                // rect may be smaller than the config rect
                                // due to aspect-ratio-preserving prescale,
                                // so re-centre within the new config rect.
                                let mut ov = (*existing).clone();
                                let cfg_w = img_cfg.transform.rect.width.cast_signed();
                                let cfg_h = img_cfg.transform.rect.height.cast_signed();
                                let ov_w = ov.rect.width.cast_signed();
                                let ov_h = ov.rect.height.cast_signed();
                                ov.rect.x = img_cfg.transform.rect.x + (cfg_w - ov_w) / 2;
                                ov.rect.y = img_cfg.transform.rect.y + (cfg_h - ov_h) / 2;
                                ov.opacity = img_cfg.transform.opacity;
                                ov.rotation_degrees = img_cfg.transform.rotation_degrees;
                                ov.z_index = img_cfg.transform.z_index;
                                new_image_overlays.push(Arc::new(ov));
                                new_cfg_indices.push(new_idx);
                                continue;
                            }
                        }
                        match decode_image_overlay(img_cfg) {
                            Ok(ov) => {
                                new_image_overlays.push(Arc::new(ov));
                                new_cfg_indices.push(new_idx);
                            },
                            Err(e) => tracing::warn!("Image overlay decode failed: {e}"),
                        }
                    }
                    *image_overlays = Arc::from(new_image_overlays);
                    *image_overlay_cfg_indices = new_cfg_indices;

                    // Re-rasterize text overlays.
                    let new_text_overlays: Vec<Arc<DecodedOverlay>> = new_config
                        .text_overlays
                        .iter()
                        .map(|txt_cfg| Arc::new(rasterize_text_overlay(txt_cfg)))
                        .collect();
                    *text_overlays = Arc::from(new_text_overlays);

                    *config = new_config;
                },
                Err(e) => {
                    tracing::warn!("Rejected invalid compositor config: {e}");
                    stats_tracker.errored();
                },
            },
            Err(e) => {
                tracing::warn!("Failed to deserialize compositor UpdateParams: {e}");
                stats_tracker.errored();
            },
        }
    }

    fn handle_pin_management(
        node: &mut Box<Self>,
        msg: PinManagementMessage,
        slots: &mut Vec<InputSlot>,
    ) {
        match msg {
            PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| {
                    let name = format!("in_{}", node.next_input_id);
                    node.next_input_id += 1;
                    name
                });
                let pin = Self::make_input_pin(pin_name);
                node.input_pins.push(pin.clone());
                let _ = response_tx.send(Ok(pin));
            },
            PinManagementMessage::AddedInputPin { pin, channel } => {
                tracing::info!("CompositorNode: activated input pin '{}'", pin.name);
                slots.push(InputSlot { name: pin.name, rx: channel, latest_frame: None });
            },
            PinManagementMessage::RemoveInputPin { pin_name } => {
                tracing::info!("CompositorNode: removed input pin '{}'", pin_name);
                slots.retain(|s| s.name != pin_name);
                node.input_pins.retain(|p| p.name != pin_name);
            },
            _ => {},
        }
    }
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_compositor_nodes(registry: &mut NodeRegistry) {
    let (def_inputs, def_outputs) = CompositorNode::definition_pins();

    registry.register_static_with_description(
        "video::compositor",
        |params| {
            let config: CompositorConfig = config_helpers::parse_config_optional(params)?;
            if let Err(e) = config.validate() {
                return Err(StreamKitError::Configuration(e));
            }
            Ok(Box::new(CompositorNode::new(config)))
        },
        serde_json::to_value(schema_for!(CompositorConfig))
            .expect("CompositorConfig schema should serialize to JSON"),
        StaticPins { inputs: def_inputs, outputs: def_outputs },
        vec!["video".to_string(), "compositing".to_string()],
        false,
        "Composites multiple raw video inputs (RGBA8) onto a single canvas with \
         image and text overlays. Supports dynamic pin creation for attaching \
         arbitrary inputs at runtime.",
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use config::{LayerConfig, Rect};
    use pixel_ops::{scale_blit_rgba, scale_blit_rgba_rotated};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    /// Create a solid-colour RGBA8 VideoFrame.
    fn make_rgba_frame(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> VideoFrame {
        let total = (width as usize) * (height as usize) * 4;
        let mut data = vec![0u8; total];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
            pixel[3] = a;
        }
        VideoFrame::new(width, height, PixelFormat::Rgba8, data).unwrap()
    }

    // ── Unit tests for compositing helpers ───────────────────────────────

    #[test]
    fn test_scale_blit_identity() {
        // 2x2 red source blitted onto a 4x4 canvas at (1,1) 2x2 rect.
        let src = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 128, 128, 255];
        let mut dst = vec![0u8; 4 * 4 * 4]; // 4x4 RGBA, all transparent black

        scale_blit_rgba(&mut dst, 4, 4, &src, 2, 2, &Rect { x: 1, y: 1, width: 2, height: 2 }, 1.0);

        // Pixel at (1,1) should be red.
        let x = 1usize;
        let y = 1usize;
        let idx = (y * 4 + x) * 4;
        assert_eq!(dst[idx], 255);
        assert_eq!(dst[idx + 1], 0);
        assert_eq!(dst[idx + 2], 0);
        assert_eq!(dst[idx + 3], 255);

        // Pixel at (0,0) should remain transparent black.
        assert_eq!(dst[0], 0);
        assert_eq!(dst[3], 0);
    }

    #[test]
    fn test_scale_blit_with_opacity() {
        // White source at 50% opacity over black background.
        let src = vec![255, 255, 255, 255]; // 1x1 white
        let mut dst = vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]; // 2x2 black

        scale_blit_rgba(&mut dst, 2, 2, &src, 1, 1, &Rect { x: 0, y: 0, width: 1, height: 1 }, 0.5);

        // Pixel (0,0): white at 50% over opaque black -> ~128 grey.
        let r = dst[0];
        assert!(r > 120 && r < 135, "Expected ~128, got {r}");
    }

    #[test]
    fn test_scale_blit_scaling() {
        // 1x1 red source scaled to 4x4 rect on an 8x8 canvas.
        let src = vec![255, 0, 0, 255];
        let mut dst = vec![0u8; 8 * 8 * 4];

        scale_blit_rgba(&mut dst, 8, 8, &src, 1, 1, &Rect { x: 2, y: 2, width: 4, height: 4 }, 1.0);

        // All pixels in the 4x4 destination rect should be red.
        for y in 2..6u32 {
            for x in 2..6u32 {
                let idx = ((y * 8 + x) * 4) as usize;
                assert_eq!(dst[idx], 255, "Red at ({x},{y})");
                assert_eq!(dst[idx + 1], 0, "Green at ({x},{y})");
            }
        }
        // Outside should remain black.
        assert_eq!(dst[0], 0);
    }

    #[test]
    fn test_rotated_blit_stretch_to_fill() {
        // A wide 4×2 red source blitted into a square 20×20 rect with 45°
        // rotation on a 40×40 canvas.
        //
        // The source is stretched to fill the 20×20 rect (no aspect-ratio
        // fit), then rotated 45°.  The centre of the rect (canvas pixel
        // 20,20) should be covered by red source pixels, while the rect
        // corner (10,10) — outside the rotated area — should remain
        // transparent.
        let src = [255u8, 0, 0, 255].repeat(4 * 2); // 4×2 solid red
        let mut dst = vec![0u8; 40 * 40 * 4];

        scale_blit_rgba_rotated(
            &mut dst,
            40,
            40,
            &src,
            4,
            2,
            &Rect { x: 10, y: 10, width: 20, height: 20 },
            1.0,
            45.0,
        );

        // The centre of the rect (canvas pixel 20,20) should be covered
        // by source content (red).
        let cx = 20usize;
        let cy = 20usize;
        let idx = (cy * 40 + cx) * 4;
        assert_eq!(dst[idx], 255, "Centre R");
        assert_eq!(dst[idx + 1], 0, "Centre G");
        assert_eq!(dst[idx + 2], 0, "Centre B");
        assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");

        // The rect corner (10,10) is outside the rotated content area
        // and should remain transparent.
        let corner_idx = (10usize * 40 + 10) * 4;
        assert_eq!(dst[corner_idx + 3], 0, "Rect corner should be transparent");
    }

    #[test]
    fn test_composite_frame_empty_layers() {
        // No layers, no overlays -> transparent black canvas.
        let mut cache = ConversionCache::new();
        let result = composite_frame(4, 4, &[], &[], None, &mut cache);
        let buf = result.as_slice();
        assert_eq!(buf.len(), 4 * 4 * 4);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_composite_frame_single_layer() {
        let data = make_rgba_frame(2, 2, 255, 0, 0, 255);
        let layer = LayerSnapshot {
            data: data.data,
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
            opacity: 1.0,
            z_index: 0,
            rotation_degrees: 0.0,
        };

        let mut cache = ConversionCache::new();
        let result = composite_frame(4, 4, &[Some(layer)], &[], None, &mut cache);
        let buf = result.as_slice();

        // Entire canvas should be red (scaled from 2x2 to 4x4).
        for pixel in buf.chunks_exact(4) {
            assert_eq!(pixel[0], 255, "Red channel");
            assert_eq!(pixel[1], 0, "Green channel");
            assert_eq!(pixel[2], 0, "Blue channel");
            assert_eq!(pixel[3], 255, "Alpha channel");
        }
    }

    #[test]
    fn test_composite_frame_two_layers() {
        // Bottom: full-canvas red. Top: small green square at (1,1) 2x2.
        let red = make_rgba_frame(4, 4, 255, 0, 0, 255);
        let green = make_rgba_frame(2, 2, 0, 255, 0, 255);

        let layer0 = LayerSnapshot {
            data: red.data,
            width: 4,
            height: 4,
            pixel_format: PixelFormat::Rgba8,
            rect: None,
            opacity: 1.0,
            z_index: 0,
            rotation_degrees: 0.0,
        };
        let layer1 = LayerSnapshot {
            data: green.data,
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 1, y: 1, width: 2, height: 2 }),
            opacity: 1.0,
            z_index: 1,
            rotation_degrees: 0.0,
        };

        let mut cache = ConversionCache::new();
        let result = composite_frame(4, 4, &[Some(layer0), Some(layer1)], &[], None, &mut cache);
        let buf = result.as_slice();

        // (0,0) should be red.
        assert_eq!(buf[0], 255);
        assert_eq!(buf[1], 0);

        // (1,1) should be green (overwritten by top layer).
        let x = 1usize;
        let y = 1usize;
        let idx = (y * 4 + x) * 4;
        assert_eq!(buf[idx], 0);
        assert_eq!(buf[idx + 1], 255);
        assert_eq!(buf[idx + 2], 0);
    }

    #[test]
    fn test_rasterize_text_overlay_produces_pixels() {
        let cfg = config::TextOverlayConfig {
            text: "Hi".to_string(),
            transform: config::OverlayTransform {
                rect: Rect { x: 0, y: 0, width: 64, height: 32 },
                opacity: 1.0,
                rotation_degrees: 0.0,
                z_index: 0,
            },
            color: [255, 255, 0, 255],
            font_size: 24,
            font_path: None,
            font_data_base64: None,
            font_name: None,
        };
        let overlay = rasterize_text_overlay(&cfg);
        // Width and height should be at least the original rect dimensions.
        assert!(overlay.width >= 64);
        assert!(overlay.height >= 32);
        // The rect in the returned overlay should match the bitmap dimensions.
        assert_eq!(overlay.rect.width, overlay.width);
        assert_eq!(overlay.rect.height, overlay.height);
        // Should have some non-zero pixels (text was drawn).
        assert!(overlay.rgba_data.iter().any(|&b| b > 0));
    }

    #[test]
    fn test_fit_rect_preserving_aspect() {
        // 4:3 source into 16:9 bounds → pillarboxed (width-limited)
        let bounds = Rect { x: 100, y: 50, width: 426, height: 240 };
        let fitted = fit_rect_preserving_aspect(640, 480, &bounds);
        // Scale = min(426/640, 240/480) = min(0.666, 0.5) = 0.5
        // Fitted: 320×240, centred within 426×240
        assert_eq!(fitted.width, 320);
        assert_eq!(fitted.height, 240);
        assert_eq!(fitted.x, 100 + (426 - 320) / 2);
        assert_eq!(fitted.y, 50);

        // 16:9 source into 4:3 bounds → letterboxed (height-limited)
        let bounds = Rect { x: 0, y: 0, width: 400, height: 400 };
        let fitted = fit_rect_preserving_aspect(1280, 720, &bounds);
        // Scale = min(400/1280, 400/720) = min(0.3125, 0.555) = 0.3125
        // Fitted: 400×225, centred within 400×400
        assert_eq!(fitted.width, 400);
        assert_eq!(fitted.height, 225);
        assert_eq!(fitted.x, 0);
        assert_eq!(fitted.y, (400 - 225) / 2);

        // Exact match → no change
        let bounds = Rect { x: 10, y: 20, width: 640, height: 480 };
        let fitted = fit_rect_preserving_aspect(640, 480, &bounds);
        assert_eq!(fitted.width, 640);
        assert_eq!(fitted.height, 480);
        assert_eq!(fitted.x, 10);
        assert_eq!(fitted.y, 20);
    }

    #[test]
    fn test_config_validate_ok() {
        let cfg = CompositorConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_zero_dimensions() {
        let cfg = CompositorConfig { width: 0, height: 720, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validate_bad_opacity() {
        let mut cfg = CompositorConfig::default();
        cfg.layers.insert("in_0".to_string(), LayerConfig { opacity: 1.5, ..Default::default() });
        assert!(cfg.validate().is_err());
    }

    // ── Integration test: node run() with mock context ──────────────────

    #[tokio::test]
    async fn test_compositor_node_run_main_only() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = CompositorConfig { width: 4, height: 4, ..Default::default() };
        let node = CompositorNode::new(config);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send a red frame.
        let frame = make_rgba_frame(2, 2, 255, 0, 0, 255);
        input_tx.send(Packet::Video(frame)).await.unwrap();

        // Give time for processing.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Close input.
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty(), "Expected at least 1 output frame");

        // Verify output is 4x4 RGBA.
        if let Packet::Video(ref out_frame) = output_packets[0] {
            assert_eq!(out_frame.width, 4);
            assert_eq!(out_frame.height, 4);
            assert_eq!(out_frame.pixel_format, PixelFormat::Rgba8);
            // Should be red (2x2 scaled to fill 4x4).
            assert_eq!(out_frame.data()[0], 255); // R
            assert_eq!(out_frame.data()[1], 0); // G
        } else {
            panic!("Expected video packet");
        }
    }

    #[tokio::test]
    async fn test_compositor_node_preserves_metadata() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = CompositorConfig { width: 2, height: 2, ..Default::default() };
        let node = CompositorNode::new(config);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let mut frame = make_rgba_frame(2, 2, 100, 100, 100, 255);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(42_000),
            duration_us: Some(33_333),
            sequence: Some(7),
            keyframe: Some(true),
        });
        input_tx.send(Packet::Video(frame)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty());

        if let Packet::Video(ref out_frame) = output_packets[0] {
            let meta = out_frame.metadata.as_ref().expect("metadata should be preserved");
            assert_eq!(meta.timestamp_us, Some(42_000));
            assert_eq!(meta.duration_us, Some(33_333));
            assert_eq!(meta.sequence, Some(0)); // output sequence starts at 0
        } else {
            panic!("Expected video packet");
        }
    }

    #[test]
    fn test_compositor_definition_pins() {
        let (inputs, outputs) = CompositorNode::definition_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert!(matches!(inputs[0].cardinality, PinCardinality::Dynamic { .. }));
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
    }

    #[test]
    fn test_compositor_pool_usage() {
        use streamkit_core::frame_pool::FramePool;

        let canvas_w = 4u32;
        let canvas_h = 4u32;
        let total = (canvas_w as usize) * (canvas_h as usize) * 4; // 64 bytes

        let pool = FramePool::<u8>::preallocated(&[total], 2);
        assert_eq!(pool.stats().buckets[0].available, 2);

        let mut cache = ConversionCache::new();
        let result = composite_frame(canvas_w, canvas_h, &[], &[], Some(&pool), &mut cache);
        assert_eq!(result.as_slice().len(), total);
        // One buffer was taken from the pool.
        assert_eq!(pool.stats().buckets[0].available, 1);

        // Drop returns to pool.
        drop(result);
        assert_eq!(pool.stats().buckets[0].available, 2);
    }

    // ── SIMD vs scalar equivalence tests ────────────────────────────────

    /// Helper: scalar I420→RGBA8 conversion for a single pixel (reference).
    #[allow(clippy::many_single_char_names)]
    fn scalar_i420_to_rgba8(y: u8, u: u8, v: u8) -> [u8; 4] {
        let c = i32::from(y) - 16;
        let d = i32::from(u) - 128;
        let e = i32::from(v) - 128;
        let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
        let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
        let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
        [r, g, b, 255]
    }

    /// Helper: scalar RGBA8→Y for a single pixel (reference).
    fn scalar_rgba8_to_y(r: u8, g: u8, b: u8) -> u8 {
        let y = ((66 * i32::from(r) + 129 * i32::from(g) + 25 * i32::from(b) + 128) >> 8) + 16;
        y.clamp(0, 255) as u8
    }

    #[test]
    fn test_i420_to_rgba8_simd_matches_scalar() {
        // Test a variety of YUV values, including edge cases that trigger
        // i16 overflow with the BT.601 coefficients.
        let test_cases: Vec<(u8, u8, u8)> = vec![
            (16, 128, 128),  // black
            (235, 128, 128), // white
            (81, 90, 240),   // pure red
            (145, 54, 34),   // pure green
            (41, 240, 110),  // pure blue
            (255, 128, 128), // max Y
            (0, 0, 0),       // min everything
            (255, 255, 255), // max everything
            (16, 0, 255),    // extreme chroma
            (235, 255, 0),   // extreme chroma
        ];

        let width = test_cases.len() as u32;
        // Build I420 buffer.
        let mut y_plane = Vec::new();
        let mut u_plane = Vec::new();
        let mut v_plane = Vec::new();
        for &(y, u, v) in &test_cases {
            y_plane.push(y);
            // Each chroma sample covers 2 luma pixels horizontally.
            if y_plane.len() % 2 == 1 {
                u_plane.push(u);
                v_plane.push(v);
            }
        }
        let chroma_w = (width as usize).div_ceil(2);
        // Pad if needed.
        while u_plane.len() < chroma_w {
            u_plane.push(128);
            v_plane.push(128);
        }

        let mut i420_data = Vec::new();
        i420_data.extend_from_slice(&y_plane);
        i420_data.extend_from_slice(&u_plane);
        i420_data.extend_from_slice(&v_plane);

        // Convert using the public function (which uses SIMD internally).
        let mut simd_out = vec![0u8; width as usize * 4];
        pixel_ops::i420_to_rgba8_buf(&i420_data, width, 1, &mut simd_out);

        // Compare with scalar reference.
        for (i, &(y, _u, _v)) in test_cases.iter().enumerate() {
            // For chroma, each sample covers 2 pixels, so use the chroma
            // value from the corresponding pair.
            let chroma_idx = i / 2;
            let actual_u = u_plane[chroma_idx];
            let actual_v = v_plane[chroma_idx];
            let expected = scalar_i420_to_rgba8(y, actual_u, actual_v);
            let got = &simd_out[i * 4..(i + 1) * 4];
            assert_eq!(
                got, &expected,
                "pixel {i}: Y={y} U={actual_u} V={actual_v} → expected {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn test_rgba8_to_i420_simd_matches_scalar() {
        // Test RGBA→Y conversion with values that trigger i16 overflow
        // (129 * 255 = 32895 > i16::MAX).
        let test_pixels: Vec<(u8, u8, u8)> = vec![
            (0, 0, 0),       // black
            (255, 255, 255), // white
            (255, 0, 0),     // red
            (0, 255, 0),     // green
            (0, 0, 255),     // blue
            (128, 128, 128), // mid grey
            (0, 254, 0),     // just below overflow threshold
            (0, 255, 0),     // at overflow threshold
        ];

        let width = test_pixels.len() as u32;
        let mut rgba_data = Vec::with_capacity(width as usize * 4);
        for &(r, g, b) in &test_pixels {
            rgba_data.extend_from_slice(&[r, g, b, 255]);
        }

        // Convert using the public function (SIMD internally).
        let i420_size = width as usize + 2 * (width as usize).div_ceil(2);
        let mut i420_out = vec![0u8; i420_size];
        pixel_ops::rgba8_to_i420_buf(&rgba_data, width, 1, &mut i420_out);

        // Check Y plane matches scalar.
        for (i, &(r, g, b)) in test_pixels.iter().enumerate() {
            let expected_y = scalar_rgba8_to_y(r, g, b);
            let got_y = i420_out[i];
            assert_eq!(
                got_y, expected_y,
                "pixel {i}: R={r} G={g} B={b} → expected Y={expected_y}, got Y={got_y}"
            );
        }
    }

    #[test]
    fn test_i420_rgba8_roundtrip_preserves_values() {
        // A full I420→RGBA8→I420 round-trip should produce values close
        // to the originals (within ±2 due to integer rounding).
        let width: u32 = 8;
        let height: u32 = 2;
        let w = width as usize;
        let h = height as usize;
        let chroma_w = w.div_ceil(2);

        // Build a simple I420 test pattern.
        let mut i420_data = vec![0u8; w * h + 2 * chroma_w * (h / 2)];
        // Y plane: gradient.
        for (i, val) in i420_data[..w * h].iter_mut().enumerate() {
            *val = (16 + (i * 219 / (w * h))) as u8;
        }
        // U/V planes: mid-range.
        let u_offset = w * h;
        let v_offset = u_offset + chroma_w * (h / 2);
        for i in 0..chroma_w * (h / 2) {
            i420_data[u_offset + i] = 128;
            i420_data[v_offset + i] = 128;
        }

        // I420 → RGBA8 → I420
        let mut rgba = vec![0u8; w * h * 4];
        pixel_ops::i420_to_rgba8_buf(&i420_data, width, height, &mut rgba);
        let mut i420_roundtrip = vec![0u8; i420_data.len()];
        pixel_ops::rgba8_to_i420_buf(&rgba, width, height, &mut i420_roundtrip);

        // Y values should be close (within ±2 of originals due to rounding).
        for (idx, orig_val) in i420_data[..w * h].iter().enumerate() {
            let orig = i32::from(*orig_val);
            let rt = i32::from(i420_roundtrip[idx]);
            assert!(
                (orig - rt).abs() <= 2,
                "Y[{idx}]: original={orig}, roundtrip={rt}, diff={}",
                (orig - rt).abs()
            );
        }
    }

    /// Test that `scale_blit_rgba` with opacity < 1.0 writes all rows correctly
    /// on a buffer wide enough to exercise the AVX2 blend path (32 pixels).
    /// This verifies the AVX2 → SSE2 → scalar cascade in `blit_row_alpha`.
    #[test]
    fn test_scale_blit_opacity_all_rows_written() {
        let w = 32usize;
        let h = 32usize;
        // Fully opaque red source.
        let src: Vec<u8> = [200, 50, 30, 255].repeat(w * h);
        // All-black destination (simulates cleared canvas).
        let mut dst = vec![0u8; w * h * 4];

        scale_blit_rgba(
            &mut dst,
            w as u32,
            h as u32,
            &src,
            w as u32,
            h as u32,
            &Rect { x: 0, y: 0, width: w as u32, height: h as u32 },
            0.9,
        );

        // Every single row should have been written to (non-zero pixels).
        for row in 0..h {
            let row_start = row * w * 4;
            let row_slice = &dst[row_start..row_start + w * 4];
            let any_written = row_slice.iter().any(|&b| b != 0);
            assert!(any_written, "Row {row} was not written to (all zeros)");

            // Verify each pixel matches the expected scalar blend.
            // opacity_u16 = (0.9 * 255 + 0.5) as u16 = 230
            // sa_eff = (255 * 230 + 128) >> 8 = 229
            // Dst is black (0), so blended = src * sa_eff / 255.
            let opacity_u16: u16 = 230;
            let sa_eff = ((255u16 * opacity_u16 + 128) >> 8).min(255);
            let expected_r = {
                let blend = 200u16 * sa_eff + 128;
                ((blend + (blend >> 8)) >> 8) as u8
            };
            let expected_g = {
                let blend = 50u16 * sa_eff + 128;
                ((blend + (blend >> 8)) >> 8) as u8
            };
            let expected_b = {
                let blend = 30u16 * sa_eff + 128;
                ((blend + (blend >> 8)) >> 8) as u8
            };
            for col in 0..w {
                let idx = row_start + col * 4;
                let got_r = dst[idx];
                let got_g = dst[idx + 1];
                let got_b = dst[idx + 2];
                let got_a = dst[idx + 3];

                // Allow ±1 for rounding differences between SIMD and scalar paths.
                assert!(
                    (i16::from(got_r) - i16::from(expected_r)).abs() <= 1,
                    "Row {row}, Col {col}: R={got_r}, expected ~{expected_r}"
                );
                assert!(
                    (i16::from(got_g) - i16::from(expected_g)).abs() <= 1,
                    "Row {row}, Col {col}: G={got_g}, expected ~{expected_g}"
                );
                assert!(
                    (i16::from(got_b) - i16::from(expected_b)).abs() <= 1,
                    "Row {row}, Col {col}: B={got_b}, expected ~{expected_b}"
                );
                assert!(got_a > 200, "Row {row}, Col {col}: A={got_a}, expected >200");
            }
        }
    }

    /// Test I420→RGBA8 AVX2 kernel correctness with a multi-row buffer wide
    /// enough to exercise the 8-pixel AVX2 path plus scalar remainder.
    /// Verifies the OOB-safe scalar chroma reads produce identical output to
    /// the scalar reference for every pixel.
    #[test]
    fn test_i420_to_rgba8_avx2_wide_multirow() {
        // 24 pixels wide = 3 AVX2 iterations (8px each) with 0 remainder.
        // 4 rows to exercise multi-row chroma subsampling.
        let width: u32 = 24;
        let height: u32 = 4;
        let w = width as usize;
        let h = height as usize;
        let chroma_w = w / 2;

        // Build a varied I420 test pattern.
        let mut i420_data = vec![0u8; w * h + 2 * chroma_w * (h / 2)];
        // Y plane: gradient across rows and columns.
        for row in 0..h {
            for col in 0..w {
                i420_data[row * w + col] = (16 + ((row * w + col) * 219) / (w * h)) as u8;
            }
        }
        // U/V planes: varying chroma values.
        let u_offset = w * h;
        let v_offset = u_offset + chroma_w * (h / 2);
        for i in 0..chroma_w * (h / 2) {
            i420_data[u_offset + i] = (64 + (i * 3) % 192) as u8;
            i420_data[v_offset + i] = (32 + (i * 7) % 224) as u8;
        }

        // Convert using the public function (dispatches to AVX2 on this machine).
        let mut simd_out = vec![0u8; w * h * 4];
        pixel_ops::i420_to_rgba8_buf(&i420_data, width, height, &mut simd_out);

        // Compare every pixel against the scalar reference.
        for row in 0..h {
            for col in 0..w {
                let luma = i420_data[row * w + col];
                let chroma_r = row / 2;
                let chroma_c = col / 2;
                let u_val = i420_data[u_offset + chroma_r * chroma_w + chroma_c];
                let v_val = i420_data[v_offset + chroma_r * chroma_w + chroma_c];
                let expected = scalar_i420_to_rgba8(luma, u_val, v_val);
                let got_idx = (row * w + col) * 4;
                let got = &simd_out[got_idx..got_idx + 4];
                assert_eq!(
                    got, &expected,
                    "row={row} col={col}: Y={luma} U={u_val} V={v_val} → expected {expected:?}, got {got:?}"
                );
            }
        }
    }

    /// Test that opacity < 1.0 through `composite_frame` produces correct
    /// output with no black borders when source matches canvas dimensions.
    #[test]
    fn test_composite_frame_opacity_no_black_borders() {
        let w = 32u32;
        let h = 32u32;
        let frame = make_rgba_frame(w, h, 200, 100, 50, 255);

        let layer = LayerSnapshot {
            data: frame.data,
            width: w,
            height: h,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 0, y: 0, width: w, height: h }),
            opacity: 0.8,
            z_index: 0,
            rotation_degrees: 0.0,
        };

        let mut cache = ConversionCache::new();
        let result = composite_frame(w, h, &[Some(layer)], &[], None, &mut cache);
        let buf = result.as_slice();

        // Every row should have non-zero content (no black borders).
        for row in 0..h as usize {
            let row_start = row * w as usize * 4;
            let row_end = row_start + w as usize * 4;
            let any_nonzero = buf[row_start..row_end].iter().any(|&b| b != 0);
            assert!(any_nonzero, "Row {row} is all zeros — black border detected");
        }
    }

    /// Full-pipeline test at real dimensions (640×480): compositor blit with
    /// opacity < 1.0, then RGBA→NV12→RGBA roundtrip, checking for black bands.
    /// This exercises the exact pipeline the VP9 encoder sees.
    #[test]
    #[allow(clippy::many_single_char_names)] // Standard image-processing shorthand (w, h, r, g, b, etc.)
    fn test_full_pipeline_opacity_nv12_roundtrip_no_black_bands() {
        let w = 640u32;
        let h = 480u32;
        let wu = w as usize;
        let hu = h as usize;

        // Create a colorbars-like pattern: 7 vertical bars of different colors.
        let colors: [(u8, u8, u8); 7] = [
            (255, 255, 255), // white
            (255, 255, 0),   // yellow
            (0, 255, 255),   // cyan
            (0, 255, 0),     // green
            (255, 0, 255),   // magenta
            (255, 0, 0),     // red
            (0, 0, 255),     // blue
        ];
        let mut src_rgba = vec![0u8; wu * hu * 4];
        for row in 0..hu {
            for col in 0..wu {
                let bar_idx = (col * 7) / wu;
                let (r, g, b) = colors[bar_idx];
                let off = (row * wu + col) * 4;
                src_rgba[off] = r;
                src_rgba[off + 1] = g;
                src_rgba[off + 2] = b;
                src_rgba[off + 3] = 255;
            }
        }

        // Step 1: Blit onto canvas with opacity 0.9 (through scale_blit_rgba_rotated,
        // exactly as the compositor does).
        let mut canvas = vec![0u8; wu * hu * 4];
        pixel_ops::scale_blit_rgba_rotated(
            &mut canvas,
            w,
            h,
            &src_rgba,
            w,
            h,
            &Rect { x: 0, y: 0, width: w, height: h },
            0.9,
            0.0,
        );

        // Verify compositor output: every row should have non-zero pixels.
        for row in 0..hu {
            let row_start = row * wu * 4;
            let any_nonzero = canvas[row_start..row_start + wu * 4].iter().any(|&b| b != 0);
            assert!(any_nonzero, "Compositor output row {row} is all zeros (black band)");
        }

        // Step 2: Convert RGBA → NV12 (exactly as the VP9 encoder does).
        let chroma_w = wu.div_ceil(2);
        let chroma_h = hu.div_ceil(2);
        let nv12_size = wu * hu + chroma_w * 2 * chroma_h;
        let mut nv12 = vec![0u8; nv12_size];
        pixel_ops::rgba8_to_nv12_buf(&canvas, w, h, &mut nv12);

        // Verify Y plane: no rows should be all-zero (Y=0 is below black level).
        // With opacity 0.9 on colored bars, Y values should be well above 0.
        for row in 0..hu {
            let y_row = &nv12[row * wu..(row + 1) * wu];
            let max_y = *y_row.iter().max().unwrap();
            assert!(max_y > 16, "NV12 Y-plane row {row}: max Y={max_y}, expected >16 (not black)");
        }

        // Step 3: Convert NV12 → RGBA (simulates decoder display).
        let mut decoded_rgba = vec![0u8; wu * hu * 4];
        pixel_ops::nv12_to_rgba8_buf(&nv12, w, h, &mut decoded_rgba);

        // Verify decoded output: every row should have non-black pixels.
        for row in 0..hu {
            let row_start = row * wu * 4;
            let row_slice = &decoded_rgba[row_start..row_start + wu * 4];
            // Check that at least some pixels have R, G, or B > 10 (not near-black).
            let has_visible =
                row_slice.chunks_exact(4).any(|px| px[0] > 10 || px[1] > 10 || px[2] > 10);
            assert!(has_visible, "Decoded row {row} has no visible pixels (all near-black)");
        }
    }

    /// Regression test: a 4:3 source blitted onto a 16:9 canvas with opacity < 1.0
    /// must cover the entire canvas (stretch-to-fill) with no black bars.
    /// Previously the near-zero rotation fast path applied an aspect-ratio-preserving
    /// fit that left letterbox gaps visible as black bands when opacity < 1.0.
    #[test]
    fn test_mismatched_aspect_ratio_opacity_no_black_bars() {
        let src_w = 640u32;
        let src_h = 480u32; // 4:3
        let canvas_w = 1280u32;
        let canvas_h = 720u32; // 16:9

        // Solid green source.
        let src = [0u8, 255, 0, 255].repeat((src_w * src_h) as usize);
        let mut canvas = vec![0u8; (canvas_w * canvas_h * 4) as usize];

        pixel_ops::scale_blit_rgba_rotated(
            &mut canvas,
            canvas_w,
            canvas_h,
            &src,
            src_w,
            src_h,
            &Rect { x: 0, y: 0, width: canvas_w, height: canvas_h },
            0.9,
            0.0, // no rotation — exercises the near-zero fast path
        );

        // Every row should have non-zero pixels (no black bars on left/right).
        for row in 0..canvas_h as usize {
            let row_start = row * canvas_w as usize * 4;
            let row_end = row_start + canvas_w as usize * 4;
            let any_nonzero = canvas[row_start..row_end].iter().any(|&b| b != 0);
            assert!(any_nonzero, "Row {row} is all zeros — black bar detected");
        }

        // Every column should have non-zero pixels (no black bars on top/bottom).
        for col in 0..canvas_w as usize {
            let any_nonzero = (0..canvas_h as usize).any(|row| {
                let idx = (row * canvas_w as usize + col) * 4;
                canvas[idx] != 0 || canvas[idx + 1] != 0 || canvas[idx + 2] != 0
            });
            assert!(any_nonzero, "Column {col} is all zeros — black bar detected");
        }
    }

    /// Regression test: a 4:3 source blitted into a non-square rect with 15°
    /// rotation must cover the centre of the rect (stretch-to-fill, not
    /// aspect-ratio fit).  Exercises the rotated path's per-axis inverse
    /// scaling (`inv_scale_x` / `inv_scale_y`).
    #[test]
    fn test_rotated_blit_mismatched_aspect_ratio_covers_centre() {
        // 4×2 red source into a 40×20 rect (2:1 aspect mismatch) at 15° on
        // a 60×40 canvas.  The centre of the rect (canvas pixel 30,20) must
        // be covered by red source content.
        let src = [255u8, 0, 0, 255].repeat(4 * 2); // 4×2 solid red
        let mut dst = vec![0u8; 60 * 40 * 4];

        scale_blit_rgba_rotated(
            &mut dst,
            60,
            40,
            &src,
            4,
            2,
            &Rect { x: 10, y: 10, width: 40, height: 20 },
            1.0,
            15.0,
        );

        // Centre of the rect (canvas pixel 30, 20) should be red.
        let cx = 30usize;
        let cy = 20usize;
        let idx = (cy * 60 + cx) * 4;
        assert_eq!(dst[idx], 255, "Centre R");
        assert_eq!(dst[idx + 1], 0, "Centre G");
        assert_eq!(dst[idx + 2], 0, "Centre B");
        assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");
    }

    /// Test RGBA→NV12 AVX2 chroma conversion matches scalar reference.
    /// Uses a 640-wide frame to fully exercise the AVX2 path (8 chroma samples/iter).
    #[test]
    #[allow(clippy::many_single_char_names)] // Standard image-processing shorthand (w, h, r, g, b, etc.)
    fn test_rgba8_to_nv12_avx2_chroma_matches_scalar() {
        let w = 640u32;
        let h = 4u32;
        let wu = w as usize;
        let hu = h as usize;
        let chroma_w = wu / 2;
        let chroma_h = hu / 2;

        // Create a varied RGBA pattern.
        let mut rgba = vec![0u8; wu * hu * 4];
        for row in 0..hu {
            for col in 0..wu {
                let off = (row * wu + col) * 4;
                rgba[off] = ((col * 3 + row * 7) % 256) as u8; // R
                rgba[off + 1] = ((col * 5 + row * 11) % 256) as u8; // G
                rgba[off + 2] = ((col * 7 + row * 13) % 256) as u8; // B
                rgba[off + 3] = 255; // A
            }
        }

        // Convert using the public function (dispatches to AVX2).
        let nv12_size = wu * hu + chroma_w * 2 * chroma_h;
        let mut nv12_simd = vec![0u8; nv12_size];
        pixel_ops::rgba8_to_nv12_buf(&rgba, w, h, &mut nv12_simd);

        // Compute scalar reference for the chroma plane.
        let y_size = wu * hu;
        for crow in 0..chroma_h {
            let r0 = crow * 2;
            for ccol in 0..chroma_w {
                let c0 = ccol * 2;
                let mut sr = 0i32;
                let mut sg = 0i32;
                let mut sb = 0i32;
                let mut count = 0i32;
                for dr in 0..2u32 {
                    let rr = r0 + dr as usize;
                    if rr >= hu {
                        continue;
                    }
                    for dc in 0..2u32 {
                        let cc = c0 + dc as usize;
                        if cc < wu {
                            let off = (rr * wu + cc) * 4;
                            sr += i32::from(rgba[off]);
                            sg += i32::from(rgba[off + 1]);
                            sb += i32::from(rgba[off + 2]);
                            count += 1;
                        }
                    }
                }
                let r_avg = sr / count;
                let g_avg = sg / count;
                let b_avg = sb / count;
                let expected_u = ((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128;
                let expected_v = ((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128;
                let expected_u = expected_u.clamp(0, 255) as u8;
                let expected_v = expected_v.clamp(0, 255) as u8;

                let uv_off = y_size + crow * chroma_w * 2 + ccol * 2;
                let got_u = nv12_simd[uv_off];
                let got_v = nv12_simd[uv_off + 1];

                // Allow ±2 for rounding differences between SIMD and scalar.
                assert!(
                    (i16::from(got_u) - i16::from(expected_u)).abs() <= 2,
                    "crow={crow} ccol={ccol}: U got={got_u}, expected={expected_u}"
                );
                assert!(
                    (i16::from(got_v) - i16::from(expected_v)).abs() <= 2,
                    "crow={crow} ccol={ccol}: V got={got_v}, expected={expected_v}"
                );
            }
        }

        // Also verify Y plane matches scalar reference.
        for row in 0..hu {
            for col in 0..wu {
                let off = (row * wu + col) * 4;
                let r = i32::from(rgba[off]);
                let g = i32::from(rgba[off + 1]);
                let b = i32::from(rgba[off + 2]);
                let expected_y =
                    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
                let got_y = nv12_simd[row * wu + col];
                assert!(
                    (i16::from(got_y) - i16::from(expected_y)).abs() <= 1,
                    "row={row} col={col}: Y got={got_y}, expected={expected_y}"
                );
            }
        }
    }
}
