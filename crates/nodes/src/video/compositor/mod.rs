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
        let (rect, opacity, z_index, rotation_degrees) = if let Some(lc) = layer_cfg {
            (lc.rect.clone(), lc.opacity, lc.z_index, lc.rotation_degrees)
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
                0.9,
                idx as i32,
                0.0,
            )
        } else {
            (None, 1.0, 0, 0.0)
        };
        configs.push(ResolvedSlotConfig { rect, opacity, z_index, rotation_degrees });
    }

    // Pre-sort by (z_index, slot_index).
    let mut draw_order: Vec<usize> = (0..num_slots).collect();
    draw_order.sort_by(|&a, &b| configs[a].z_index.cmp(&configs[b].z_index).then(a.cmp(&b)));

    (configs, draw_order)
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
        let mut image_overlays_vec: Vec<Arc<DecodedOverlay>> =
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

        // ── Cached layer config + draw order ────────────────────────────
        // Rebuilt only when config or pin set changes (UpdateParams,
        // pin add/remove, channel close).  Avoids per-frame HashMap
        // lookups and sort_by calls.
        let mut layer_configs_dirty = true;
        let mut resolved_configs: Vec<ResolvedSlotConfig> = Vec::new();
        let mut sorted_draw_order: Vec<usize> = Vec::new();

        loop {
            // ── Take at most one frame from every slot (non-blocking) ───
            // We intentionally take only one frame per slot per iteration so
            // that every produced frame is composited and forwarded.  The old
            // "drain-to-latest" approach dropped intermediate frames when the
            // compositing step was slower than the producer.
            let mut got_any_frame = false;
            for slot in &mut slots {
                if let Ok(Packet::Video(frame)) = slot.rx.try_recv() {
                    slot.latest_frame = Some(frame);
                    got_any_frame = true;
                }
            }

            // ── Wait for at least one frame if none are available yet ────
            if !got_any_frame && !slots.is_empty() {
                // Use select! to wait for any input, control, or pin management.
                let mut received_frame = false;
                let mut should_break = false;

                tokio::select! {
                    biased;

                    // Control messages (highest priority).
                    Some(ctrl_msg) = context.control_rx.recv() => {
                        match ctrl_msg {
                            NodeControlMessage::Shutdown => {
                                tracing::info!("CompositorNode received shutdown");
                                should_break = true;
                            },
                            NodeControlMessage::UpdateParams(params) => {
                                Self::apply_update_params(
                                    &mut self.config,
                                    &mut image_overlays,
                                    &mut text_overlays,
                                    params,
                                    &mut stats_tracker,
                                );
                                layer_configs_dirty = true;
                            },
                            NodeControlMessage::Start => {},
                        }
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
                    }

                    // Wait for a frame from any connected input.
                    result = recv_from_any_slot(&mut slots) => {
                        match result {
                            SlotRecvResult::Frame(slot_idx, frame) => {
                                slots[slot_idx].latest_frame = Some(frame);
                                received_frame = true;
                            }
                            SlotRecvResult::ChannelClosed(slot_idx) => {
                                tracing::info!(
                                    "CompositorNode: input '{}' closed",
                                    slots[slot_idx].name
                                );
                                slots.remove(slot_idx);
                                layer_configs_dirty = true;
                                if slots.is_empty() {
                                    stop_reason = "all_inputs_closed";
                                    should_break = true;
                                }
                                // Otherwise continue — remaining slots are still active.
                            }
                            SlotRecvResult::NonVideo(slot_idx) => {
                                tracing::debug!(
                                    "CompositorNode: ignoring non-video packet on '{}'",
                                    slots[slot_idx].name
                                );
                                // Skip and continue waiting.
                            }
                            SlotRecvResult::Empty => {
                                stop_reason = "all_inputs_closed";
                                should_break = true;
                            }
                        }
                    }
                }

                if should_break {
                    break;
                }
                if !received_frame {
                    continue;
                }
            }

            if slots.is_empty() {
                // No inputs at all — wait for pin management or control.
                tokio::select! {
                    Some(ctrl_msg) = context.control_rx.recv() => {
                        match ctrl_msg {
                            NodeControlMessage::Shutdown => {
                                tracing::info!("CompositorNode received shutdown (no inputs)");
                                break;
                            },
                            NodeControlMessage::UpdateParams(params) => {
                                Self::apply_update_params(
                                    &mut self.config,
                                    &mut image_overlays,
                                    &mut text_overlays,
                                    params,
                                    &mut stats_tracker,
                                );
                                layer_configs_dirty = true;
                            },
                            NodeControlMessage::Start => {},
                        }
                    }
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
                    }
                }
                continue;
            }

            // ── Check for non-blocking control / pin management ──────────
            let mut should_stop = false;
            while let Ok(ctrl_msg) = context.control_rx.try_recv() {
                match ctrl_msg {
                    NodeControlMessage::Shutdown => {
                        tracing::info!("CompositorNode received shutdown during compositing");
                        stop_reason = "shutdown";
                        should_stop = true;
                        break;
                    },
                    NodeControlMessage::UpdateParams(params) => {
                        Self::apply_update_params(
                            &mut self.config,
                            &mut image_overlays,
                            &mut text_overlays,
                            params,
                            &mut stats_tracker,
                        );
                        layer_configs_dirty = true;
                    },
                    NodeControlMessage::Start => {},
                }
            }
            if should_stop {
                break;
            }
            if let Some(ref mut pmrx) = pin_mgmt_rx {
                while let Ok(msg) = pmrx.try_recv() {
                    Self::handle_pin_management(&mut self, msg, &mut slots);
                    layer_configs_dirty = true;
                }
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
                        LayerSnapshot {
                            data: f.data.clone(),
                            width: f.width,
                            height: f.height,
                            pixel_format: f.pixel_format,
                            rect: cfg.rect.clone(),
                            opacity: cfg.opacity,
                            z_index: cfg.z_index,
                            rotation_degrees: cfg.rotation_degrees,
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

            if work_tx.send(work_item).await.is_err() {
                tracing::debug!("Compositing thread gone, stopping CompositorNode");
                stop_reason = "compositor_thread_gone";
                break;
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
                keyframe: Some(true),
            });

            let out_frame = VideoFrame::from_pooled(
                self.config.width,
                self.config.height,
                PixelFormat::Rgba8,
                composite_result.rgba_data,
                metadata,
            );

            if context.output_sender.send("out", Packet::Video(out_frame)).await.is_err() {
                tracing::debug!("Output channel closed, stopping CompositorNode");
                stop_reason = "output_closed";
                break;
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
                    // We match cached overlays by content identity rather
                    // than positional index because failed decodes cause the
                    // decoded slice to be shorter than the config vec,
                    // making positional lookups incorrect.
                    let old_imgs = image_overlays.clone();
                    let old_cfgs = &config.image_overlays;

                    // Build a content-keyed cache from the successfully
                    // decoded old overlays.  We match each decoded bitmap
                    // back to its originating config by walking old_cfgs
                    // and old_imgs together: for each old config we check
                    // whether the next decoded overlay's dimensions match
                    // the config's target rect (a successful decode
                    // prescales to exactly those dimensions).  On mismatch
                    // we skip the config (it must have been a failed
                    // decode) without consuming a decoded entry.
                    //
                    // The cache key is (data_base64, width, height) and
                    // values are Vec to handle duplicate images at the
                    // same size.
                    type CacheKey<'a> = (&'a str, u32, u32);
                    let mut cache: HashMap<CacheKey<'_>, Vec<Arc<DecodedOverlay>>> = HashMap::new();
                    let mut dec_iter = old_imgs.iter().peekable();
                    for old_cfg in old_cfgs.iter() {
                        if let Some(decoded) = dec_iter.peek() {
                            let tw = old_cfg.transform.rect.width;
                            let th = old_cfg.transform.rect.height;
                            // A successfully decoded overlay is prescaled
                            // to the config's target dimensions.  If the
                            // next decoded bitmap's size matches we know
                            // it belongs to this config; otherwise this
                            // config's decode must have failed.
                            if decoded.width == tw && decoded.height == th {
                                let key: CacheKey<'_> = (&old_cfg.data_base64, tw, th);
                                cache
                                    .entry(key)
                                    .or_default()
                                    .push(Arc::clone(dec_iter.next().expect("peeked")));
                            }
                        }
                    }

                    let mut new_image_overlays: Vec<Arc<DecodedOverlay>> =
                        Vec::with_capacity(new_config.image_overlays.len());
                    for img_cfg in &new_config.image_overlays {
                        let key: CacheKey<'_> = (
                            &img_cfg.data_base64,
                            img_cfg.transform.rect.width,
                            img_cfg.transform.rect.height,
                        );
                        if let Some(entries) = cache.get_mut(&key) {
                            if let Some(existing) = entries.pop() {
                                // Content and target dimensions unchanged —
                                // reuse the decoded bitmap, just update
                                // mutable transform fields.
                                let mut ov = (*existing).clone();
                                ov.rect = img_cfg.transform.rect.clone();
                                ov.opacity = img_cfg.transform.opacity;
                                ov.rotation_degrees = img_cfg.transform.rotation_degrees;
                                ov.z_index = img_cfg.transform.z_index;
                                new_image_overlays.push(Arc::new(ov));
                                continue;
                            }
                        }
                        match decode_image_overlay(img_cfg) {
                            Ok(ov) => new_image_overlays.push(Arc::new(ov)),
                            Err(e) => tracing::warn!("Image overlay decode failed: {e}"),
                        }
                    }
                    *image_overlays = Arc::from(new_image_overlays);

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

// ── Frame receive helper ────────────────────────────────────────────────────

/// Result of waiting for a frame from input slots.
enum SlotRecvResult {
    /// A video frame was received from the slot at the given index.
    Frame(usize, VideoFrame),
    /// The channel at the given index was closed.
    ChannelClosed(usize),
    /// A non-video packet was received (and discarded) from the given index.
    NonVideo(usize),
    /// All slots are empty (should not happen if caller checks).
    Empty,
}

/// Wait for a packet from any of the input slots.  Returns a typed result so
/// the caller can distinguish between a received video frame, a closed channel,
/// and a non-video packet (which should be skipped, not treated as closure).
///
/// Uses `poll_recv` directly to avoid per-call allocations from boxing
/// futures and collecting them into a `Vec` for `select_all`.
async fn recv_from_any_slot(slots: &mut [InputSlot]) -> SlotRecvResult {
    if slots.is_empty() {
        return SlotRecvResult::Empty;
    }

    std::future::poll_fn(|cx| {
        for (i, slot) in slots.iter_mut().enumerate() {
            match slot.rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(Packet::Video(frame))) => {
                    return std::task::Poll::Ready(SlotRecvResult::Frame(i, frame));
                },
                std::task::Poll::Ready(Some(_)) => {
                    return std::task::Poll::Ready(SlotRecvResult::NonVideo(i));
                },
                std::task::Poll::Ready(None) => {
                    return std::task::Poll::Ready(SlotRecvResult::ChannelClosed(i));
                },
                std::task::Poll::Pending => {},
            }
        }
        std::task::Poll::Pending
    })
    .await
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
        VideoFrame::new(width, height, PixelFormat::Rgba8, data)
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
        let result = composite_frame(4, 4, &[], &[], &[], None, &mut cache);
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
        let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
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
        let result =
            composite_frame(4, 4, &[Some(layer0), Some(layer1)], &[], &[], None, &mut cache);
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
        };
        let overlay = rasterize_text_overlay(&cfg);
        assert_eq!(overlay.width, 64);
        assert_eq!(overlay.height, 32);
        // Should have some non-zero pixels (text was drawn).
        assert!(overlay.rgba_data.iter().any(|&b| b > 0));
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
        let result = composite_frame(canvas_w, canvas_h, &[], &[], &[], Some(&pool), &mut cache);
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
        let result = composite_frame(w, h, &[Some(layer)], &[], &[], None, &mut cache);
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
