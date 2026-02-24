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
//! - Text overlays are rasterized via `tiny-skia` once per `UpdateParams`, not
//!   per frame.
//!
//! # Future work
//! - GPU-accelerated compositing via `wgpu`.
//! - Bilinear / Lanczos scaling (MVP uses nearest-neighbor).

pub mod config;
mod kernel;
mod overlay;
mod pixel_ops;

use async_trait::async_trait;
use config::{CompositorConfig, Rect};
use futures::future::select_all;
use kernel::{CompositeResult, CompositeWorkItem, LayerSnapshot};
use overlay::{decode_image_overlay, rasterize_text_overlay, DecodedOverlay};
use pixel_ops::rgba8_to_i420_buf;
use schemars::schema_for;
use std::pin::Pin;
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

use config::parse_pixel_format;
use kernel::composite_frame;

// ── Input slot ──────────────────────────────────────────────────────────────

/// Holds a receiver and the most-recently-received frame for one input layer.
struct InputSlot {
    name: String,
    rx: mpsc::Receiver<Packet>,
    latest_frame: Option<VideoFrame>,
}

// ── Node ────────────────────────────────────────────────────────────────────

/// Composites multiple raw video inputs onto a single RGBA8 canvas with
/// optional image/text overlays.
///
/// Inputs are dynamic (`PinCardinality::Dynamic`) and can be attached at
/// runtime. Each input accepts `RawVideo(RGBA8)` with wildcard dimensions.
///
/// Output `"out"` produces `RawVideo` at the configured canvas size and
/// pixel format (RGBA8 by default, or I420 if `output_pixel_format` is set).
pub struct CompositorNode {
    config: CompositorConfig,
    /// Resolved output pixel format.
    output_format: PixelFormat,
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

        let output_format =
            parse_pixel_format(&config.output_pixel_format).unwrap_or(PixelFormat::Rgba8);

        Self { config, output_format, input_pins, next_input_id }
    }

    /// Returns the definition-time pins for registry (dynamic template).
    pub fn definition_pins() -> (Vec<InputPin>, Vec<OutputPin>) {
        let inputs = vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
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
            ],
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
            accepts_types: vec![
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
            ],
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
                pixel_format: self.output_format,
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
        let mut image_overlays: Vec<Arc<DecodedOverlay>> =
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
                    image_overlays.push(Arc::new(overlay));
                },
                Err(e) => {
                    tracing::warn!("Failed to decode image overlay {}: {}", i, e);
                },
            }
        }

        // Rasterize text overlays (once; re-done on UpdateParams).  Also Arc-wrapped.
        let mut text_overlays: Vec<Arc<DecodedOverlay>> =
            Vec::with_capacity(self.config.text_overlays.len());
        for txt_cfg in &self.config.text_overlays {
            text_overlays.push(Arc::new(rasterize_text_overlay(txt_cfg)));
        }

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
        let pre_inputs: Vec<(String, mpsc::Receiver<Packet>)> = context.inputs.drain().collect();
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
            // Persistent scratch buffer for I420→RGBA8 layer conversion,
            // reused across frames to avoid per-frame allocation.
            let mut i420_to_rgba_scratch: Vec<u8> = Vec::new();

            while let Some(work) = work_rx.blocking_recv() {
                // Fast path: try I420 pass-through to skip the entire
                // I420 → RGBA8 → I420 round-trip.  The passthrough returns
                // the index of the qualifying layer; we copy its data into
                // a pooled buffer (a cheap memcpy vs. two colour-space
                // conversions + compositing).
                if let Some(pt_idx) = kernel::try_i420_passthrough(
                    work.canvas_w,
                    work.canvas_h,
                    &work.layers,
                    &work.image_overlays,
                    &work.text_overlays,
                    work.output_format,
                ) {
                    let src = work.layers[pt_idx].as_ref().unwrap().data.as_slice();
                    let pooled = if let Some(ref pool) = work.video_pool {
                        let mut p = pool.get(src.len());
                        p.as_mut_slice()[..src.len()].copy_from_slice(src);
                        p
                    } else {
                        streamkit_core::frame_pool::PooledVideoData::from_vec(src.to_vec())
                    };
                    let result = CompositeResult {
                        output_format: work.output_format,
                        rgba_data: None,
                        i420_data: Some(pooled),
                    };
                    if result_tx.blocking_send(result).is_err() {
                        break;
                    }
                    continue;
                }

                let rgba_buf = composite_frame(
                    work.canvas_w,
                    work.canvas_h,
                    &work.layers,
                    &work.image_overlays,
                    &work.text_overlays,
                    work.video_pool.as_deref(),
                    &mut i420_to_rgba_scratch,
                );
                let result = if work.output_format == PixelFormat::I420 {
                    // Convert RGBA8 → I420 directly into a pooled buffer
                    // (no intermediate scratch — avoids a full extra memcpy).
                    let w = work.canvas_w as usize;
                    let h = work.canvas_h as usize;
                    let chroma_w = (w + 1) / 2;
                    let chroma_h = (h + 1) / 2;
                    let i420_size = w * h + 2 * chroma_w * chroma_h;
                    let mut i420_pooled = if let Some(ref pool) = work.video_pool {
                        pool.get(i420_size)
                    } else {
                        streamkit_core::frame_pool::PooledVideoData::from_vec(vec![0u8; i420_size])
                    };
                    rgba8_to_i420_buf(
                        rgba_buf.as_slice(),
                        work.canvas_w,
                        work.canvas_h,
                        i420_pooled.as_mut_slice(),
                    );
                    CompositeResult {
                        output_format: work.output_format,
                        rgba_data: None,
                        i420_data: Some(i420_pooled),
                    }
                } else {
                    CompositeResult {
                        output_format: work.output_format,
                        rgba_data: Some(rgba_buf),
                        i420_data: None,
                    }
                };
                if result_tx.blocking_send(result).is_err() {
                    break;
                }
            }
        });

        let mut output_seq: u64 = 0;
        let mut stop_reason: &str = "shutdown";

        loop {
            // ── Take at most one frame from every slot (non-blocking) ───
            // We intentionally take only one frame per slot per iteration so
            // that every produced frame is composited and forwarded.  The old
            // "drain-to-latest" approach dropped intermediate frames when the
            // compositing step was slower than the producer.
            let mut got_any_frame = false;
            for slot in &mut slots {
                if let Ok(packet) = slot.rx.try_recv() {
                    if let Packet::Video(frame) = packet {
                        slot.latest_frame = Some(frame);
                        got_any_frame = true;
                    }
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
                    }

                    // Wait for a frame from any connected input.
                    result = recv_from_any_slot(&mut slots) => {
                        if let Some((slot_idx, frame)) = result {
                            slots[slot_idx].latest_frame = Some(frame);
                            received_frame = true;
                        } else {
                            // All inputs closed.
                            stop_reason = "all_inputs_closed";
                            should_break = true;
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
                }
            }

            // ── Send work to persistent compositing thread ─────────────
            // Collect the data we need to send to the blocking thread.
            let num_slots = slots.len();
            let layers: Vec<Option<LayerSnapshot>> = slots
                .iter()
                .enumerate()
                .map(|(idx, slot)| {
                    slot.latest_frame.as_ref().map(|f| {
                        let layer_cfg = self.config.layers.get(&slot.name);
                        let (rect, opacity) = if let Some(lc) = layer_cfg {
                            // Explicit per-layer config.
                            (lc.rect.clone(), lc.opacity)
                        } else if idx > 0 && num_slots > 1 {
                            // Auto-PiP: non-first layers without explicit config
                            // are placed in the bottom-right corner at 1/3 canvas
                            // size with slight transparency.
                            let pip_w = self.config.width / 3;
                            let pip_h = self.config.height / 3;
                            let pip_x = self.config.width - pip_w - 20;
                            let pip_y = self.config.height - pip_h - 20;
                            (Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }), 0.9)
                        } else {
                            // First layer (or single input): fill the canvas.
                            (None, 1.0)
                        };
                        LayerSnapshot {
                            data: f.data.clone(),
                            width: f.width,
                            height: f.height,
                            pixel_format: f.pixel_format,
                            rect,
                            opacity,
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
                output_format: self.output_format,
            };

            if work_tx.send(work_item).await.is_err() {
                tracing::debug!("Compositing thread gone, stopping CompositorNode");
                stop_reason = "compositor_thread_gone";
                break;
            }

            let composite_result = match result_rx.recv().await {
                Some(r) => r,
                None => {
                    tracing::debug!("Compositing result channel closed");
                    stop_reason = "compositor_thread_gone";
                    break;
                },
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

            let out_frame = if composite_result.output_format == PixelFormat::I420 {
                let i420_pooled =
                    composite_result.i420_data.expect("I420 output data must be present");
                VideoFrame::from_pooled(
                    self.config.width,
                    self.config.height,
                    PixelFormat::I420,
                    i420_pooled,
                    metadata,
                )
            } else {
                let pooled = composite_result.rgba_data.expect("RGBA8 output data must be present");
                VideoFrame::from_pooled(
                    self.config.width,
                    self.config.height,
                    PixelFormat::Rgba8,
                    pooled,
                    metadata,
                )
            };

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
        image_overlays: &mut Vec<Arc<DecodedOverlay>>,
        text_overlays: &mut Vec<Arc<DecodedOverlay>>,
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

                    // Always re-decode image overlays (content may have changed
                    // even if the count is the same).
                    image_overlays.clear();
                    for img_cfg in &new_config.image_overlays {
                        match decode_image_overlay(img_cfg) {
                            Ok(ov) => image_overlays.push(Arc::new(ov)),
                            Err(e) => tracing::warn!("Image overlay decode failed: {e}"),
                        }
                    }

                    // Re-rasterize text overlays.
                    text_overlays.clear();
                    for txt_cfg in &new_config.text_overlays {
                        text_overlays.push(Arc::new(rasterize_text_overlay(txt_cfg)));
                    }

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

/// Wait for a video frame from any of the input slots. Returns the slot index
/// and the received frame, or `None` if all channels are closed.
async fn recv_from_any_slot(slots: &mut [InputSlot]) -> Option<(usize, VideoFrame)> {
    if slots.is_empty() {
        return None;
    }

    // Use futures to poll all receivers concurrently.
    type SlotRecvFut<'a> =
        Pin<Box<dyn futures::Future<Output = (usize, Option<Packet>)> + Send + 'a>>;

    let futs: Vec<SlotRecvFut<'_>> = slots
        .iter_mut()
        .enumerate()
        .map(|(i, slot)| {
            let fut = async move {
                let pkt = slot.rx.recv().await;
                (i, pkt)
            };
            Box::pin(fut) as Pin<Box<dyn futures::Future<Output = _> + Send + '_>>
        })
        .collect();

    if futs.is_empty() {
        return None;
    }

    let (result, _idx, _remaining) = select_all(futs).await;
    let (slot_idx, maybe_packet) = result;

    maybe_packet.and_then(|pkt| match pkt {
        Packet::Video(frame) => Some((slot_idx, frame)),
        _ => None,
    })
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used)]
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
    use config::LayerConfig;
    use pixel_ops::scale_blit_rgba;
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
        let idx = (1 * 4 + 1) * 4;
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
    fn test_composite_frame_empty_layers() {
        // No layers, no overlays -> transparent black canvas.
        let mut scratch = Vec::new();
        let result = composite_frame(4, 4, &[], &[], &[], None, &mut scratch);
        let buf = result.as_slice();
        assert_eq!(buf.len(), 4 * 4 * 4);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_composite_frame_single_layer() {
        let data = make_rgba_frame(2, 2, 255, 0, 0, 255);
        let layer = LayerSnapshot {
            data: data.data.clone(),
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
            opacity: 1.0,
        };

        let mut scratch = Vec::new();
        let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut scratch);
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
            data: red.data.clone(),
            width: 4,
            height: 4,
            pixel_format: PixelFormat::Rgba8,
            rect: None,
            opacity: 1.0,
        };
        let layer1 = LayerSnapshot {
            data: green.data.clone(),
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 1, y: 1, width: 2, height: 2 }),
            opacity: 1.0,
        };

        let mut scratch = Vec::new();
        let result =
            composite_frame(4, 4, &[Some(layer0), Some(layer1)], &[], &[], None, &mut scratch);
        let buf = result.as_slice();

        // (0,0) should be red.
        assert_eq!(buf[0], 255);
        assert_eq!(buf[1], 0);

        // (1,1) should be green (overwritten by top layer).
        let idx = (1 * 4 + 1) * 4;
        assert_eq!(buf[idx], 0);
        assert_eq!(buf[idx + 1], 255);
        assert_eq!(buf[idx + 2], 0);
    }

    #[test]
    fn test_rasterize_text_overlay_produces_pixels() {
        let cfg = config::TextOverlayConfig {
            text: "Hi".to_string(),
            rect: Rect { x: 0, y: 0, width: 64, height: 32 },
            color: [255, 255, 0, 255],
            font_size: 24,
            opacity: 1.0,
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
        cfg.layers.insert("in_0".to_string(), LayerConfig { rect: None, opacity: 1.5 });
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

        let mut scratch = Vec::new();
        let result = composite_frame(canvas_w, canvas_h, &[], &[], &[], Some(&pool), &mut scratch);
        assert_eq!(result.as_slice().len(), total);
        // One buffer was taken from the pool.
        assert_eq!(pool.stats().buckets[0].available, 1);

        // Drop returns to pool.
        drop(result);
        assert_eq!(pool.stats().buckets[0].available, 2);
    }
}
