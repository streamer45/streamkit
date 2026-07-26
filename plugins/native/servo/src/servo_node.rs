// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! `NativeSourceNode` implementation for the Servo web renderer plugin.

use std::time::{Duration, Instant};

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    PacketMetadata, PixelFormat, RawVideoFormat, VideoFrame,
};

use crate::config::ServoConfig;
use crate::servo_thread::{send_work, NodeId, ServoThreadResult, ServoWorkItem};

/// How long after the first paint the first-load gate keeps waiting for
/// `LoadStatus::Complete` before starting emission anyway.  Ad-heavy pages
/// routinely delay the load event by tens of seconds (or never fire it)
/// while the content is already rendered; painted-plus-settled captures
/// that content without waiting out the full `load_timeout_secs`.
const POST_PAINT_SETTLE: Duration = Duration::from_secs(2);

/// Interval between status polls while the first-load gate holds emission.
const LOAD_POLL_INTERVAL: Duration = Duration::from_millis(50);

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
    awaiting_first_load: bool,
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
                    "viewport_width": {
                        "type": "integer",
                        "default": 0,
                        "description": "Browser viewport width (0 = same as output width). Set larger to see more of the page, scaled down."
                    },
                    "viewport_height": {
                        "type": "integer",
                        "default": 0,
                        "description": "Browser viewport height (0 = same as output height). Set larger to see more of the page, scaled down."
                    },
                    "viewport_resolution": {
                        "type": "string",
                        "description": "Viewport resolution preset (WxH). Overrides viewport_width/viewport_height at runtime.",
                        "tunable": true,
                        "enum": ["640x480", "1280x720", "1280x960", "1920x1080", "2560x1440"]
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
                        "description": "Maximum seconds to hold frame emission while the initial page becomes ready. Emission starts on load-complete, or ~2s after the first paint when the load event lags (ad-heavy pages may never fire it); on timeout it starts regardless. The wait happens inside the first tick, so keep this below the host's native_call_timeout_secs.",
                        "minimum": 1
                    },
                    "auth": {
                        "type": "object",
                        "description": "Init-time authentication for loading private pages. Applied once at WebView creation; not hot-swappable. Credentials are never logged.",
                        "tunable": false,
                        "properties": {
                            "headers": {
                                "type": "object",
                                "description": "Arbitrary request headers attached to every navigation, including runtime URL changes (e.g. Authorization, Cookie). Values are credentials.",
                                "additionalProperties": { "type": "string" }
                            },
                            "bearer_token": {
                                "type": "string",
                                "description": "Convenience for 'Authorization: Bearer <token>'. Conflicts with an explicit Authorization header."
                            },
                            "basic": {
                                "type": "object",
                                "description": "HTTP Basic/Digest credentials answered non-interactively on an auth challenge.",
                                "properties": {
                                    "username": { "type": "string" },
                                    "password": { "type": "string" }
                                },
                                "required": ["username", "password"]
                            },
                            "user_agent": {
                                "type": "string",
                                "description": "Custom User-Agent. NOTE: Servo preferences are process-global, so this applies to ALL servo nodes in the process."
                            }
                        }
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
        let mut config: ServoConfig = if let Some(p) = params {
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
                awaiting_first_load: false,
                logger,
            });
        };

        // Apply viewport_resolution preset at init time so that
        // viewport_width/viewport_height are populated before validation
        // and rendering context creation.
        config.apply_resolution_preset();

        config.validate()?;

        let fps = config.fps.max(1);
        let duration_us = 1_000_000 / u64::from(fps);

        plugin_info!(
            logger,
            "Initializing Servo plugin: {}x{} (viewport {}x{}) @ {} fps, url='{}'",
            config.width,
            config.height,
            config.effective_viewport_width(),
            config.effective_viewport_height(),
            fps,
            crate::config::redact_url(&config.url)
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
                Ok(Self {
                    config,
                    node_id,
                    result_rx,
                    tick_count: 0,
                    duration_us,
                    awaiting_first_load: true,
                    logger,
                })
            },
            Ok(ServoThreadResult::InitErr(e)) => {
                Err(format!("Servo instance creation failed: {e}"))
            },
            Ok(_) => Err("Unexpected result during init".to_string()),
            Err(_) => Err("Shared Servo thread channel closed during init".to_string()),
        }
    }

    fn tick(&mut self, output: &OutputSender) -> Result<bool, String> {
        if self.awaiting_first_load {
            self.await_first_load()?;
            self.awaiting_first_load = false;
        }

        let Some(rgba_data) = self.request_frame()? else {
            return Ok(false);
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
            plugin_info!(
                self.logger,
                "Updated Servo config (url='{}')",
                crate::config::redact_url(&self.config.url)
            );
        }
        Ok(())
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
            let width = width.clamp(1, crate::config::MAX_DIMENSION);
            let height = height.clamp(1, crate::config::MAX_DIMENSION);
            if width == self.config.width && height == self.config.height {
                return;
            }

            plugin_info!(
                self.logger,
                "Upstream hint: resizing output from {}x{} to {}x{}",
                self.config.width,
                self.config.height,
                width,
                height
            );

            self.config.width = width;
            self.config.height = height;

            let _ = send_work(ServoWorkItem::Resize { node_id: self.node_id, width, height });
        }
    }

    fn cleanup(&mut self) {
        let _ = send_work(ServoWorkItem::Unregister { node_id: self.node_id });
        plugin_info!(self.logger, "Servo instance unregistered: {}", self.node_id);
    }
}

impl ServoSourcePlugin {
    /// Request one rendered frame from the shared Servo thread.
    ///
    /// Returns `Ok(None)` on an unexpected (non-frame) result so the
    /// caller can skip the tick without failing the node.
    fn request_frame(&self) -> Result<Option<Vec<u8>>, String> {
        send_work(ServoWorkItem::Render { node_id: self.node_id })?;
        match self.result_rx.recv() {
            Ok(ServoThreadResult::Frame { rgba_data }) => Ok(Some(rgba_data)),
            Ok(_) => {
                plugin_warn!(self.logger, "Unexpected result from Servo thread");
                Ok(None)
            },
            Err(_) => Err("Servo thread result channel closed".to_string()),
        }
    }

    /// Request the page's `(painted, loaded)` state from the shared Servo
    /// thread.  Each poll pumps the shared event loop (which is what
    /// advances the load) without a full-frame readback.
    ///
    /// Returns `Ok(None)` on an unexpected (non-status) result so the
    /// caller can bail out without failing the node.
    fn request_status(&self) -> Result<Option<(bool, bool)>, String> {
        send_work(ServoWorkItem::Status { node_id: self.node_id })?;
        match self.result_rx.recv() {
            Ok(ServoThreadResult::Status { painted, loaded }) => Ok(Some((painted, loaded))),
            Ok(_) => {
                plugin_warn!(self.logger, "Unexpected result from Servo thread");
                Ok(None)
            },
            Err(_) => Err("Servo thread result channel closed".to_string()),
        }
    }

    /// Hold the first emitted frame until the page is ready, so captures
    /// start at real content rather than blank/loading frames.
    ///
    /// Ready means `LoadStatus::Complete`, or [`POST_PAINT_SETTLE`] after
    /// the first paint when the load event lags behind visible content
    /// (ad-heavy pages may never fire it), capped by `load_timeout_secs`.
    /// Only this node's tick loop blocks; the shared Servo thread keeps
    /// serving other instances between polls.
    fn await_first_load(&self) -> Result<(), String> {
        let deadline =
            Instant::now() + Duration::from_secs(u64::from(self.config.load_timeout_secs));
        let mut first_paint: Option<Instant> = None;
        loop {
            let Some((painted, loaded)) = self.request_status()? else {
                return Ok(());
            };
            if loaded {
                plugin_info!(self.logger, "Initial page loaded; starting frame emission");
                return Ok(());
            }
            if painted
                && first_paint.get_or_insert_with(Instant::now).elapsed() >= POST_PAINT_SETTLE
            {
                plugin_info!(
                    self.logger,
                    "Initial page painted but load still pending after {:?}; \
                     starting frame emission",
                    POST_PAINT_SETTLE
                );
                return Ok(());
            }
            if Instant::now() >= deadline {
                plugin_warn!(
                    self.logger,
                    "Page did not finish loading within {}s; starting frame emission anyway",
                    self.config.load_timeout_secs
                );
                return Ok(());
            }
            std::thread::sleep(LOAD_POLL_INTERVAL);
        }
    }
}
