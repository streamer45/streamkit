// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! `NativeSourceNode` implementation for the Slint video source plugin.

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    PacketMetadata, PixelFormat, RawVideoFormat, VideoFrame,
};

use crate::config::SlintConfig;
use crate::slint_thread::{
    send_work, DiscoveredProperty, DiscoveredValueType, NodeId, SlintThreadResult, SlintWorkItem,
};

/// Slint UI video source plugin.
///
/// Renders `.slint` files to RGBA8 video frames at a configurable resolution
/// and frame rate.  All Slint operations run on a shared dedicated thread;
/// this struct holds the channel handle and per-instance state.
pub struct SlintSourcePlugin {
    config: SlintConfig,
    node_id: NodeId,
    result_rx: std::sync::mpsc::Receiver<SlintThreadResult>,
    tick_count: u64,
    duration_us: u64,
    logger: Logger,
    /// Properties discovered from the compiled `.slint` component at init.
    /// Used to build the runtime param schema so the UI can render controls.
    discovered_properties: Vec<DiscoveredProperty>,
}

impl NativeSourceNode for SlintSourcePlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("slint")
            .output(
                "out",
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Rgba8,
                }),
            )
            .category("video")
            .category("generators")
            .description(
                "Renders a Slint UI component into RGBA8 video frames. \
                 Compiles a .slint file at init and produces frames at the \
                 configured resolution and frame rate. Properties can be \
                 updated at runtime via UpdateParams.",
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "width": {
                        "type": "integer",
                        "default": 640,
                        "description": "Output frame width in pixels",
                        "minimum": 1
                    },
                    "height": {
                        "type": "integer",
                        "default": 480,
                        "description": "Output frame height in pixels",
                        "minimum": 1
                    },
                    "fps": {
                        "type": "integer",
                        "default": 30,
                        "description": "Output frame rate",
                        "minimum": 1
                    },
                    "slint_file": {
                        "type": "string",
                        "description": "Path to the .slint file"
                    },
                    "component": {
                        "type": "string",
                        "description": "Name of the exported component to instantiate (defaults to first)"
                    },
                    "properties": {
                        "type": "object",
                        "default": {},
                        "description": "Key-value map of Slint properties (strings, numbers, booleans)"
                    },
                    "property_keyframes": {
                        "type": "array",
                        "default": [],
                        "description": "List of property snapshots to cycle through over time",
                        "items": { "type": "object" }
                    },
                    "keyframe_interval": {
                        "type": "integer",
                        "default": 90,
                        "description": "Frames between keyframe switches",
                        "minimum": 1
                    },
                    "frame_count": {
                        "type": "integer",
                        "default": 0,
                        "description": "Total frames to generate (0 = infinite)"
                    },
                    "static_ui": {
                        "type": "boolean",
                        "default": false,
                        "description": "Cache frames when properties haven't changed"
                    }
                },
                "required": ["slint_file"]
            }))
            .build()
    }

    fn source_config(&self) -> SourceConfig {
        let fps = self.config.fps.max(1);
        if self.config.frame_count > 0 {
            SourceConfig {
                tick_interval_us: 1_000_000 / u64::from(fps),
                max_ticks: u64::from(self.config.frame_count),
            }
        } else {
            SourceConfig::from_fps(fps)
        }
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        let config: SlintConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Invalid config: {e}"))?
        } else {
            // Parameterless construction is used by the host to probe
            // source_config().  Return a lightweight default instance
            // without starting the Slint thread.
            let config = SlintConfig::default();
            let (_tx, result_rx) = std::sync::mpsc::sync_channel(1);
            return Ok(Self {
                config,
                node_id: uuid::Uuid::new_v4(),
                result_rx,
                tick_count: 0,
                duration_us: 1_000_000 / 30,
                logger,
                discovered_properties: Vec::new(),
            });
        };

        config.validate()?;

        let fps = config.fps.max(1);
        let duration_us = 1_000_000 / u64::from(fps);

        plugin_info!(
            logger,
            "Initializing Slint plugin: {}x{} @ {} fps, slint_file='{}'",
            config.width,
            config.height,
            fps,
            config.slint_file
        );

        let node_id = uuid::Uuid::new_v4();

        // Use a bounded channel with capacity 2 to allow one frame in-flight
        // plus the init result, without unbounded buffering.
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(2);

        // Register on the shared Slint thread.
        send_work(SlintWorkItem::Register { node_id, config: config.clone(), result_tx })?;

        // Wait for init result.
        match result_rx.recv() {
            Ok(SlintThreadResult::InitOk { properties }) => {
                plugin_info!(logger, "Slint instance registered: {node_id}");
                Ok(Self {
                    config,
                    node_id,
                    result_rx,
                    tick_count: 0,
                    duration_us,
                    logger,
                    discovered_properties: properties,
                })
            },
            Ok(SlintThreadResult::InitErr(e)) => {
                Err(format!("Slint instance creation failed: {e}"))
            },
            Ok(SlintThreadResult::Frame { .. }) => {
                Err("Unexpected frame result during init".to_string())
            },
            Err(_) => Err("Shared Slint thread channel closed during init".to_string()),
        }
    }

    fn tick(&mut self, output: &OutputSender) -> Result<bool, String> {
        // Request a frame from the shared Slint thread.
        send_work(SlintWorkItem::Render { node_id: self.node_id })?;

        // Wait for the rendered frame.
        let rgba_data = match self.result_rx.recv() {
            Ok(SlintThreadResult::Frame { rgba_data }) => rgba_data,
            Ok(_) => {
                plugin_warn!(self.logger, "Unexpected result from Slint thread");
                return Ok(false);
            },
            Err(_) => {
                return Err("Slint thread result channel closed".to_string());
            },
        };

        let timestamp_us = self.tick_count * self.duration_us;
        let metadata = Some(PacketMetadata {
            timestamp_us: Some(timestamp_us),
            duration_us: Some(self.duration_us),
            sequence: Some(self.tick_count),
            keyframe: Some(true),
        });

        // Try zero-copy pool allocation; fall back to legacy copy path.
        if let Some(mut buf) = output.alloc_video(rgba_data.len()) {
            buf.as_mut_slice()[..rgba_data.len()].copy_from_slice(&rgba_data);
            output.send_video(
                "out",
                self.config.width,
                self.config.height,
                PixelFormat::Rgba8,
                buf,
                metadata.as_ref(),
            )?;
        } else {
            let frame = VideoFrame::with_metadata(
                self.config.width,
                self.config.height,
                PixelFormat::Rgba8,
                rgba_data,
                metadata,
            )
            .map_err(|e| format!("Failed to create video frame: {e}"))?;

            output.send("out", &Packet::Video(frame))?;
        }

        self.tick_count += 1;
        Ok(false)
    }

    fn update_params(&mut self, params: Option<serde_json::Value>) -> Result<(), String> {
        if let Some(p) = params {
            let update: SlintConfig =
                serde_json::from_value(p).map_err(|e| format!("Invalid params: {e}"))?;
            self.config.merge_update(&update);
            send_work(SlintWorkItem::UpdateConfig {
                node_id: self.node_id,
                config: self.config.clone(),
            })?;
            plugin_info!(self.logger, "Updated Slint properties");
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        let _ = send_work(SlintWorkItem::Unregister { node_id: self.node_id });
        plugin_info!(self.logger, "Slint instance unregistered: {}", self.node_id);
    }

    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        if self.discovered_properties.is_empty() {
            return None;
        }

        let mut props = serde_json::Map::new();
        for dp in &self.discovered_properties {
            let type_str = match dp.value_type {
                DiscoveredValueType::Bool => "boolean",
                DiscoveredValueType::Number => "number",
                DiscoveredValueType::String => "string",
            };

            let mut schema = serde_json::json!({
                "type": type_str,
                "tunable": true,
                "path": format!("properties.{}", dp.name),
                "description": format!("Slint property: {}", dp.name),
            });

            // Include the initial value from the component so the UI can
            // show the correct default state (e.g. a toggle that starts on).
            if let Some(ref initial) = dp.initial_value {
                schema["default"] = initial.clone();
            }

            props.insert(dp.name.clone(), schema);
        }

        Some(serde_json::json!({
            "type": "object",
            "properties": props,
        }))
    }

    fn on_upstream_hint(
        &mut self,
        hint: streamkit_plugin_sdk_native::streamkit_core::UpstreamHint,
    ) {
        if let streamkit_plugin_sdk_native::streamkit_core::UpstreamHint::PreferredSize {
            width,
            height,
        } = hint
        {
            // Clamp to safe bounds before checking for no-op.
            let width = width.clamp(1, crate::config::MAX_DIMENSION);
            let height = height.clamp(1, crate::config::MAX_DIMENSION);
            // Ignore if dimensions unchanged.
            if width == self.config.width && height == self.config.height {
                return;
            }

            plugin_info!(
                self.logger,
                "Upstream hint: resizing from {}x{} to {}x{}",
                self.config.width,
                self.config.height,
                width,
                height
            );

            self.config.width = width;
            self.config.height = height;

            // Tell the Slint thread to resize window + buffer.
            // FIFO ordering guarantees Resize is processed before the
            // next Render, so tick() will produce a correctly-sized frame.
            let _ = send_work(SlintWorkItem::Resize { node_id: self.node_id, width, height });
        }
        // Unknown hint variants are silently ignored (non_exhaustive enum).
    }
}
