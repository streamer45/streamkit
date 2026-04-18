// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! `NativeSourceNode` implementation for the Servo web renderer plugin.

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    PacketMetadata, PixelFormat, RawVideoFormat, VideoFrame,
};

use crate::config::ServoConfig;
use crate::servo_thread::{send_work, NodeId, ServoThreadResult, ServoWorkItem};

/// Servo web renderer video source plugin.
///
/// Renders web pages to RGBA8 video frames at a configurable resolution
/// and frame rate.  All Servo operations run on a shared dedicated thread;
/// this struct holds the channel handle and per-instance state.
pub struct ServoSourcePlugin {
    config: ServoConfig,
    node_id: NodeId,
    result_rx: std::sync::mpsc::Receiver<ServoThreadResult>,
    tick_count: u64,
    duration_us: u64,
    logger: Logger,
}

impl NativeSourceNode for ServoSourcePlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("servo")
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
                "Renders a web page into RGBA8 video frames via the Servo \
                 browser engine. Navigates to the configured URL and produces \
                 frames at the specified resolution and frame rate.",
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL of the web page to render",
                        "tunable": true
                    },
                    "width": {
                        "type": "integer",
                        "default": 1280,
                        "description": "Output frame width in pixels",
                        "minimum": 1
                    },
                    "height": {
                        "type": "integer",
                        "default": 720,
                        "description": "Output frame height in pixels",
                        "minimum": 1
                    },
                    "fps": {
                        "type": "integer",
                        "default": 30,
                        "description": "Output frame rate",
                        "minimum": 1
                    },
                    "custom_css": {
                        "type": "string",
                        "description": "Optional CSS to inject into the page",
                        "tunable": true
                    },
                    "frame_count": {
                        "type": "integer",
                        "default": 0,
                        "description": "Total frames to generate (0 = infinite)"
                    },
                    "load_timeout_secs": {
                        "type": "integer",
                        "default": 30,
                        "description": "Maximum seconds to wait for page load",
                        "minimum": 1
                    }
                },
                "required": ["url"]
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
        let config: ServoConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Invalid config: {e}"))?
        } else {
            // Parameterless construction is used by the host to probe
            // source_config().  Return a lightweight default instance
            // without starting the Servo thread.
            let config = ServoConfig::default();
            let (_tx, result_rx) = std::sync::mpsc::sync_channel(1);
            return Ok(Self {
                config,
                node_id: uuid::Uuid::new_v4(),
                result_rx,
                tick_count: 0,
                duration_us: 1_000_000 / 30,
                logger,
            });
        };

        config.validate()?;

        let fps = config.fps.max(1);
        let duration_us = 1_000_000 / u64::from(fps);

        plugin_info!(
            logger,
            "Initializing Servo plugin: {}x{} @ {} fps, url='{}'",
            config.width,
            config.height,
            fps,
            config.url
        );

        let node_id = uuid::Uuid::new_v4();

        // Use a bounded channel with capacity 2 to allow one frame in-flight
        // plus the init result, without unbounded buffering.
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(2);

        // Register on the shared Servo thread.
        send_work(ServoWorkItem::Register { node_id, config: config.clone(), result_tx })?;

        // Wait for init result.
        match result_rx.recv() {
            Ok(ServoThreadResult::InitOk) => {
                plugin_info!(logger, "Servo instance registered: {node_id}");
                Ok(Self { config, node_id, result_rx, tick_count: 0, duration_us, logger })
            },
            Ok(ServoThreadResult::InitErr(e)) => {
                Err(format!("Servo instance creation failed: {e}"))
            },
            Ok(ServoThreadResult::Frame { .. }) => {
                Err("Unexpected frame result during init".to_string())
            },
            Err(_) => Err("Shared Servo thread channel closed during init".to_string()),
        }
    }

    fn tick(&mut self, output: &OutputSender) -> Result<bool, String> {
        // Request a frame from the shared Servo thread.
        send_work(ServoWorkItem::Render { node_id: self.node_id })?;

        // Wait for the rendered frame.
        let rgba_data = match self.result_rx.recv() {
            Ok(ServoThreadResult::Frame { rgba_data }) => rgba_data,
            Ok(_) => {
                plugin_warn!(self.logger, "Unexpected result from Servo thread");
                return Ok(false);
            },
            Err(_) => {
                return Err("Servo thread result channel closed".to_string());
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
            let update: ServoConfig =
                serde_json::from_value(p).map_err(|e| format!("Invalid params: {e}"))?;
            self.config.merge_update(&update);
            send_work(ServoWorkItem::UpdateConfig {
                node_id: self.node_id,
                config: self.config.clone(),
            })?;
            plugin_info!(self.logger, "Updated Servo config (url='{}')", self.config.url);
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        let _ = send_work(ServoWorkItem::Unregister { node_id: self.node_id });
        plugin_info!(self.logger, "Servo instance unregistered: {}", self.node_id);
    }
}
