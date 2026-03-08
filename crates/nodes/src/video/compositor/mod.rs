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
use config::{CompositorConfig, CompositorLayout, ResolvedLayer, ResolvedOverlay};
use kernel::{CompositeResult, CompositeWorkItem, LayerSnapshot};
use opentelemetry::{global, KeyValue};
use overlay::{decode_image_overlay, rasterize_text_overlay, DecodedOverlay};
use schemars::schema_for;
use smallvec::SmallVec;
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
    config_helpers, state_helpers, view_data_helpers, InputPin, NodeContext, NodeRegistry,
    OutputPin, PinCardinality, ProcessorNode, StreamKitError,
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
    mirror_horizontal: bool,
    mirror_vertical: bool,
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
        let (rect, opacity, z_index, rotation_degrees, aspect_fit, mirror_h, mirror_v) =
            if let Some(lc) = layer_cfg {
                (
                    lc.rect.clone(),
                    lc.opacity,
                    lc.z_index,
                    lc.rotation_degrees,
                    false,
                    lc.mirror_horizontal,
                    lc.mirror_vertical,
                )
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
                    false,
                    false,
                )
            } else {
                (None, 1.0, 0, 0.0, false, false, false)
            };
        configs.push(ResolvedSlotConfig {
            rect,
            opacity,
            z_index,
            rotation_degrees,
            aspect_fit,
            mirror_horizontal: mirror_h,
            mirror_vertical: mirror_v,
        });
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
                    &work.image_overlays,
                    &work.text_overlays,
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
        let mut resolved_configs: Vec<ResolvedSlotConfig> = Vec::new();
        let mut sorted_draw_order: Vec<usize> = Vec::new();

        // ── View data (server-driven layout) ────────────────────────────
        let view_data_tx = context.view_data_tx.clone();
        let mut last_layout: Option<CompositorLayout> = None;

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

                // Emit layout via view data if it changed.
                let layout = Self::build_layout(
                    &self.config,
                    &slots,
                    &resolved_configs,
                    &image_overlays,
                    &text_overlays,
                );
                if last_layout.as_ref() != Some(&layout) {
                    if let Ok(json) = serde_json::to_value(&layout) {
                        view_data_helpers::emit_view_data(&view_data_tx, &node_name, json);
                    }
                    last_layout = Some(layout);
                }
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
                            mirror_horizontal: cfg.mirror_horizontal,
                            mirror_vertical: cfg.mirror_vertical,
                        }
                    })
                })
                .collect();

            stats_tracker.received();

            let work_item = CompositeWorkItem {
                canvas_w: self.config.width,
                canvas_h: self.config.height,
                layers,
                image_overlays: image_overlays.clone(),
                text_overlays: text_overlays.clone(),
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
    /// Build a `CompositorLayout` from the current resolved state.
    ///
    /// This captures the server-computed positions and dimensions for all
    /// layers and overlays, which the frontend uses as the source of truth
    /// in Monitor view.
    fn build_layout(
        config: &CompositorConfig,
        slots: &[InputSlot],
        resolved_configs: &[ResolvedSlotConfig],
        image_overlays: &Arc<[Arc<DecodedOverlay>]>,
        text_overlays: &Arc<[Arc<DecodedOverlay>]>,
    ) -> CompositorLayout {
        let mut layers: SmallVec<[ResolvedLayer; 8]> = SmallVec::new();
        for (idx, slot) in slots.iter().enumerate() {
            if let Some(cfg) = resolved_configs.get(idx) {
                let (x, y, width, height) = if cfg.aspect_fit {
                    // When aspect_fit is enabled (e.g. auto-PiP), emit the
                    // fitted rect that matches what the compositor actually
                    // renders, not the raw bounding box.  This prevents the
                    // frontend from displaying a stretched layer rectangle.
                    match (cfg.rect.as_ref(), slot.latest_frame.as_ref()) {
                        (Some(rect), Some(frame)) => {
                            let fitted =
                                fit_rect_preserving_aspect(frame.width, frame.height, rect);
                            (fitted.x, fitted.y, fitted.width, fitted.height)
                        },
                        (Some(rect), None) => (rect.x, rect.y, rect.width, rect.height),
                        _ => (0, 0, config.width, config.height),
                    }
                } else {
                    cfg.rect.as_ref().map_or((0, 0, config.width, config.height), |rect| {
                        (rect.x, rect.y, rect.width, rect.height)
                    })
                };
                layers.push(ResolvedLayer {
                    id: slot.name.clone(),
                    x,
                    y,
                    width,
                    height,
                    opacity: cfg.opacity,
                    z_index: cfg.z_index,
                    rotation_degrees: cfg.rotation_degrees,
                    mirror_horizontal: cfg.mirror_horizontal,
                    mirror_vertical: cfg.mirror_vertical,
                });
            }
        }

        let mut resolved_image_overlays: SmallVec<[ResolvedOverlay; 8]> = SmallVec::new();
        for (i, ov) in image_overlays.iter().enumerate() {
            resolved_image_overlays.push(ResolvedOverlay {
                index: i,
                x: ov.rect.x,
                y: ov.rect.y,
                width: ov.rect.width,
                height: ov.rect.height,
                opacity: ov.opacity,
                z_index: ov.z_index,
                rotation_degrees: ov.rotation_degrees,
                mirror_horizontal: ov.mirror_horizontal,
                mirror_vertical: ov.mirror_vertical,
                measured_text_width: None,
                measured_text_height: None,
            });
        }

        let mut resolved_text_overlays: SmallVec<[ResolvedOverlay; 8]> = SmallVec::new();
        for (i, ov) in text_overlays.iter().enumerate() {
            resolved_text_overlays.push(ResolvedOverlay {
                index: i,
                x: ov.rect.x,
                y: ov.rect.y,
                width: ov.rect.width,
                height: ov.rect.height,
                opacity: ov.opacity,
                z_index: ov.z_index,
                rotation_degrees: ov.rotation_degrees,
                mirror_horizontal: ov.mirror_horizontal,
                mirror_vertical: ov.mirror_vertical,
                measured_text_width: ov.measured_text_width,
                measured_text_height: ov.measured_text_height,
            });
        }

        CompositorLayout {
            canvas_width: config.width,
            canvas_height: config.height,
            layers,
            text_overlays: resolved_text_overlays,
            image_overlays: resolved_image_overlays,
        }
    }

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
                                ov.mirror_horizontal = img_cfg.transform.mirror_horizontal;
                                ov.mirror_vertical = img_cfg.transform.mirror_vertical;
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
#[path = "tests.rs"]
mod tests;
