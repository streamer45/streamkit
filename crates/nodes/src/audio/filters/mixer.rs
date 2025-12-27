// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::{atomic::AtomicBool, atomic::Ordering, Mutex};
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::types::PacketMetadata;
use streamkit_core::types::{AudioFormat, AudioFrame, Packet, PacketType, SampleFormat};
use streamkit_core::AudioFramePool;
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[serde(default)]
pub struct ClockedMixerConfig {
    /// Output sample rate (Hz). Inputs are expected to already match this.
    pub sample_rate: u32,

    /// Fixed frame size (samples per channel) for the clocked mixer.
    ///
    /// Example: `960` @ `48000` Hz => 20ms frames.
    pub frame_samples_per_channel: usize,

    /// Per-input jitter buffer depth (in frames).
    ///
    /// Frames are queued in order. When full, the oldest frame is dropped (overwrite-oldest).
    ///
    /// Recommended: 2-3 for ~40-60ms jitter tolerance at 20ms frames.
    pub jitter_buffer_frames: usize,

    /// If true, emit silence frames on ticks even when no inputs have data.
    ///
    /// If false, the clocked mixer only emits output on ticks where at least one input
    /// contributes a frame.
    pub generate_silence: bool,
}

impl Default for ClockedMixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            frame_samples_per_channel: 960,
            jitter_buffer_frames: 3,
            generate_silence: true,
        }
    }
}

/// Configuration for the AudioMixerNode.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[serde(default)]
pub struct AudioMixerConfig {
    /// Timeout in milliseconds for waiting for slow inputs.
    /// If specified, the mixer will wait up to this duration for all active pins to provide frames.
    /// If timeout expires, missing pins will be mixed as silence.
    /// If not specified (None), the mixer will wait indefinitely (strict broadcast synchronization).
    /// Default: Some(100)
    pub sync_timeout_ms: Option<u64>,

    /// Number of input pins to pre-create.
    /// Required for stateless/oneshot pipelines where pins must exist before graph building.
    /// Optional for dynamic pipelines where pins are created on-demand.
    /// If specified, pins will be named in_0, in_1, ..., in_{N-1}.
    pub num_inputs: Option<usize>,

    /// Enable clocked mixing mode (dedicated mixing thread + per-input jitter buffers).
    ///
    /// When enabled, the mixer emits frames on a fixed cadence determined by
    /// `sample_rate` and `frame_samples_per_channel`.
    pub clocked: Option<ClockedMixerConfig>,
}

impl Default for AudioMixerConfig {
    fn default() -> Self {
        // Default to 100ms timeout (5 frames at 20ms frame size)
        // This provides tolerance for timing jitter, GC pauses, and network variation
        // while still catching truly slow/stuck inputs quickly enough
        // Tests use 100ms and it works well in practice
        Self { sync_timeout_ms: Some(100), num_inputs: None, clocked: None }
    }
}

/// A node that mixes multiple raw audio streams into a single stream.
/// This node operates on 32-bit floating-point audio.
///
/// The mixer operates in dynamic mode, supporting runtime pin creation and removal.
/// It implements broadcast synchronization: waits for all active pins to provide frames
/// before mixing (with optional timeout for slow inputs).
///
/// **EOF Handling**: When a pin receives EOF (e.g., file playback completes), it is
/// automatically removed from the active pin set, and mixing continues with remaining pins.
///
/// **Output Format Stability**: The mixer tracks the maximum channel count it has observed
/// and never decreases output channels thereafter. This avoids downstream glitches when a
/// higher-channel input ends and only lower-channel inputs remain (e.g., continued speech
/// after a stereo music track completes). Mono inputs are upmixed when output is stereo.
pub struct AudioMixerNode {
    config: AudioMixerConfig,
    /// Current input pins (may grow dynamically)
    input_pins: Vec<InputPin>,
    /// Next input ID for dynamic pin naming
    next_input_id: usize,
}

struct InputSlot {
    name: Arc<str>,
    rx: mpsc::Receiver<Packet>,
    has_sent: bool,
    slow: bool,
    frame: Option<AudioFrame>,
}

impl AudioMixerNode {
    pub fn new(config: AudioMixerConfig) -> Self {
        let (input_pins, next_input_id) = config.num_inputs.map_or_else(
            || {
                // Dynamic mode - start with no pins
                (Vec::new(), 0)
            },
            |num_inputs| {
                // Pre-create pins for stateless/oneshot pipelines
                let mut pins = Vec::with_capacity(num_inputs);
                for i in 0..num_inputs {
                    pins.push(InputPin {
                        name: format!("in_{i}"),
                        accepts_types: vec![PacketType::RawAudio(AudioFormat {
                            sample_rate: 0,
                            channels: 0,
                            sample_format: SampleFormat::F32,
                        })],
                        cardinality: PinCardinality::One,
                    });
                }
                (pins, num_inputs)
            },
        );

        Self { config, input_pins, next_input_id }
    }

    /// Returns the static pins for node definition registration.
    /// For dynamic mode, this includes a Dynamic cardinality pin template.
    pub fn definition_pins() -> (Vec<InputPin>, Vec<OutputPin>) {
        let inputs = vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::RawAudio(AudioFormat {
                sample_rate: 0, // Wildcard
                channels: 0,    // Wildcard
                sample_format: SampleFormat::F32,
            })],
            cardinality: PinCardinality::Dynamic { prefix: "in".to_string() },
        }];

        let outputs = vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: 0,
                channels: 0,
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        }];

        (inputs, outputs)
    }
}

#[async_trait]
impl ProcessorNode for AudioMixerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        self.input_pins.clone()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: 0,
                channels: 0,
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    async fn run(mut self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("AudioMixerNode starting with broadcast synchronization");
        if let Some(timeout) = self.config.sync_timeout_ms {
            tracing::info!("Sync timeout: {}ms", timeout);
        } else {
            tracing::info!("Sync timeout: infinite (strict synchronization)");
        }

        if let Some(clocked) = self.config.clocked.clone() {
            tracing::info!(
                "AudioMixerNode using clocked mode ({} Hz, {} samples/ch, jitter_buffer_frames={}, generate_silence={})",
                clocked.sample_rate,
                clocked.frame_samples_per_channel,
                clocked.jitter_buffer_frames,
                clocked.generate_silence
            );
            self.run_clocked(context, clocked).await
        } else {
            self.run_dynamic(context).await
        }
    }
}

impl AudioMixerNode {
    #[allow(clippy::too_many_lines)]
    async fn run_clocked(
        mut self,
        mut context: NodeContext,
        clocked: ClockedMixerConfig,
    ) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut pin_mgmt_rx = context.pin_management_rx.take();
        let cancellation_token = context.cancellation_token.clone();

        let frame_samples_per_channel = clocked.frame_samples_per_channel.max(1);
        let jitter_buffer_frames = clocked.jitter_buffer_frames.max(1);
        let clocked_sample_rate = clocked.sample_rate;
        let clocked_generate_silence = clocked.generate_silence;
        let tick_duration = {
            let nanos_per_sec = 1_000_000_000u64;
            let nanos = (frame_samples_per_channel as u64).saturating_mul(nanos_per_sec)
                / u64::from(clocked_sample_rate.max(1));
            std::time::Duration::from_nanos(nanos.max(1))
        };

        let output_mailbox = Arc::new(OutputMailbox::new());
        let (audio_cmd_tx, audio_cmd_rx) = std::sync::mpsc::channel::<AudioThreadCommand>();

        // Audio thread drives mixing and writes frames to output_mailbox.
        let audio_pool = context.audio_pool.clone();
        let state_tx = context.state_tx.clone();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = stop_flag.clone();
        let output_mailbox_thread = output_mailbox.clone();

        let sync_timeout = self.config.sync_timeout_ms.map(std::time::Duration::from_millis);

        let node_name_thread = node_name.clone();
        let audio_thread = std::thread::Builder::new()
            .name(format!("skit-audio-mixer-{node_name}"))
            .spawn(move || {
                let config = ClockedThreadConfig {
                    node_name: node_name_thread,
                    sample_rate: clocked_sample_rate,
                    frame_samples_per_channel,
                    tick_duration,
                    generate_silence: clocked_generate_silence,
                    sync_timeout,
                    audio_pool,
                    state_tx,
                    output_mailbox: output_mailbox_thread,
                    cmd_rx: audio_cmd_rx,
                    stop_flag: stop_flag_thread,
                };
                run_clocked_audio_thread(&config);
            })
            .map_err(|e| {
                StreamKitError::Runtime(format!("Failed to spawn audio mixer thread: {e}"))
            })?;

        // Output forwarder (Tokio task) owns the OutputSender and sends frames downstream.
        let mut output_sender = context.output_sender;
        let output_mailbox_forwarder = output_mailbox.clone();
        let mut shutdown_rx = output_mailbox_forwarder.shutdown_rx.clone();
        let output_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = output_mailbox_forwarder.notify.notified() => {
                        let frame = output_mailbox_forwarder.take_latest();
                        if let Some(frame) = frame {
                            if output_sender.send("out", Packet::Audio(frame)).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // Drainers report EOF back to this management loop.
        let (input_event_tx, mut input_event_rx) = mpsc::channel::<InputEvent>(32);
        let mut drainers: HashMap<Arc<str>, tokio::task::JoinHandle<()>> = HashMap::new();
        let mut stop_reason: &'static str = "shutdown";

        for (pin_name, rx) in context.inputs {
            let (name, handle) = start_clocked_input_drainer(
                &audio_cmd_tx,
                &input_event_tx,
                cancellation_token.clone(),
                jitter_buffer_frames,
                clocked_sample_rate,
                pin_name,
                rx,
            );
            drainers.insert(name, handle);
        }

        // Main control loop: pin management, shutdown, EOF removals.
        loop {
            tokio::select! {
                Some(msg) = async {
                    match &mut pin_mgmt_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match msg {
                        PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                            let pin_name = suggested_name.unwrap_or_else(|| {
                                let name = format!("in_{}", self.next_input_id);
                                self.next_input_id += 1;
                                name
                            });

                            let pin = InputPin {
                                name: pin_name.clone(),
                                accepts_types: vec![PacketType::RawAudio(AudioFormat {
                                    sample_rate: 0,
                                    channels: 0,
                                    sample_format: SampleFormat::F32,
                                })],
                                cardinality: PinCardinality::One,
                            };

                            self.input_pins.push(pin.clone());
                            let _ = response_tx.send(Ok(pin));
                        }
                        PinManagementMessage::AddedInputPin { pin, channel } => {
                            tracing::info!("Mixer (clocked): Activated input pin {}", pin.name);
                            let (name, handle) = start_clocked_input_drainer(
                                &audio_cmd_tx,
                                &input_event_tx,
                                cancellation_token.clone(),
                                jitter_buffer_frames,
                                clocked_sample_rate,
                                pin.name,
                                channel,
                            );
                            drainers.insert(name, handle);
                        }
                        PinManagementMessage::RemoveInputPin { pin_name } => {
                            tracing::info!("Mixer (clocked): Removed input pin {}", pin_name);
                            let key: Arc<str> = Arc::from(pin_name.clone());
                            if let Some(handle) = drainers.remove(&key) {
                                handle.abort();
                            }
                            let _ = audio_cmd_tx.send(AudioThreadCommand::RemoveInput { name: key });
                            self.input_pins.retain(|p| p.name != pin_name);

                            if drainers.is_empty() {
                                stop_reason = "all_inputs_closed";
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("AudioMixerNode shutting down (shutdown requested)");
                        break;
                    }
                }

                Some(event) = input_event_rx.recv() => {
                    match event {
                        InputEvent::Eof(name) => {
                            tracing::info!("Mixer (clocked): Input {} reached EOF", name);
                            let _ = drainers.remove(&name);
                            let _ = audio_cmd_tx.send(AudioThreadCommand::RemoveInput { name });
                            if drainers.is_empty() {
                                stop_reason = "all_inputs_closed";
                                break;
                            }
                        }
                    }
                }
            }
        }

        stop_flag.store(true, Ordering::Relaxed);
        let _ = audio_cmd_tx.send(AudioThreadCommand::Shutdown);
        output_mailbox.shutdown();

        let _ = audio_thread.join();
        output_forwarder.abort();

        state_helpers::emit_stopped(&context.state_tx, &node_name, stop_reason);
        Ok(())
    }

    /// Runtime pin management with broadcast synchronization
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)] // Dynamic audio mixing logic is inherently complex
    async fn run_dynamic(mut self, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_running(&context.state_tx, &node_name);

        // Take the pin management channel (optional - only needed for fully dynamic mode)
        // In stateless pipelines with num_inputs specified, this will be None and that's OK
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        // Extract cancellation token before moving inputs
        let cancellation_token = context.cancellation_token.clone();

        let mut slots: Vec<InputSlot> = Vec::new();

        // Round-robin index for fair receiver polling
        let mut round_robin_idx: usize = 0;

        // Pre-create input pins for any connections that were provided at startup
        // This ensures compatibility with dynamic pipeline definitions that provide initial connections
        // Skip if pins were already created by the constructor (num_inputs was specified)
        if self.config.num_inputs.is_none() {
            for pin_name in context.inputs.keys() {
                let pin = InputPin {
                    name: pin_name.clone(),
                    accepts_types: vec![PacketType::RawAudio(AudioFormat {
                        sample_rate: 0,
                        channels: 0,
                        sample_format: SampleFormat::F32,
                    })],
                    cardinality: PinCardinality::One,
                };
                self.input_pins.push(pin);
                tracing::info!("Mixer: Pre-created input pin {} for initial connection", pin_name);

                // Update next_input_id if this is a numbered pin (in_N)
                if let Some(num_str) = pin_name.strip_prefix("in_") {
                    if let Ok(num) = num_str.parse::<usize>() {
                        self.next_input_id = self.next_input_id.max(num + 1);
                    }
                }
            }
        }

        // Start with inputs from context (if any were pre-connected)
        for (name, rx) in context.inputs {
            slots.push(InputSlot {
                name: Arc::from(name),
                rx,
                has_sent: false,
                slow: false,
                frame: None,
            });
        }

        // Track the maximum observed channel count; never decreases (format stability).
        let mut max_output_channels_seen: u16 = 0;

        // Track last mix time for timeout detection
        let mut waiting_since: Option<std::time::Instant> = None;
        let mut has_warned_slow = false;

        // Track mixed frames sent (for debugging)
        let mut mixed_frame_count: u64 = 0;
        let mut mix_frames: Vec<AudioFrame> = Vec::new();

        loop {
            // Determine if we have a timeout configured
            let sync_timeout = self.config.sync_timeout_ms.map(tokio::time::Duration::from_millis);

            tokio::select! {
                // Handle pin management messages (only in fully dynamic mode)
                Some(msg) = async {
                    match &mut pin_mgmt_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await, // Never completes if no channel
                    }
                } => {
                    match msg {
                        PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                            // Create new input pin
                            let pin_name = suggested_name.unwrap_or_else(|| {
                                let name = format!("in_{}", self.next_input_id);
                                self.next_input_id += 1;
                                name
                            });

                            let pin = InputPin {
                                name: pin_name.clone(),
                                accepts_types: vec![PacketType::RawAudio(AudioFormat {
                                    sample_rate: 0,
                                    channels: 0,
                                    sample_format: SampleFormat::F32,
                                })],
                                cardinality: PinCardinality::One,
                            };

                            self.input_pins.push(pin.clone());
                            tracing::info!("Mixer: Created input pin {}", pin_name);
                            let _ = response_tx.send(Ok(pin));
                        }

                        PinManagementMessage::AddedInputPin { pin, channel } => {
                            // Engine has created the channel, start receiving
                            tracing::info!("Mixer: Activated input pin {}", pin.name);
                            slots.push(InputSlot {
                                name: Arc::from(pin.name),
                                rx: channel,
                                has_sent: false,
                                slow: false,
                                frame: None,
                            });
                        }

                        PinManagementMessage::RemoveInputPin { pin_name } => {
                            // Remove input pin
                            tracing::info!("Mixer: Removed input pin {}", pin_name);
                            if let Some(idx) =
                                slots.iter().position(|s| s.name.as_ref() == pin_name.as_str())
                            {
                                slots.remove(idx);
                                if round_robin_idx > idx {
                                    round_robin_idx = round_robin_idx.saturating_sub(1);
                                }
                                if slots.is_empty() {
                                    round_robin_idx = 0;
                                } else {
                                    round_robin_idx %= slots.len();
                                }
                                waiting_since = None;
                            }
                            self.input_pins.retain(|p| p.name != pin_name);
                        }

                        _ => {
                            // Ignore output pin messages (we don't support dynamic outputs)
                        }
                    }
                    }

                    // Support explicit shutdown via control message.
                    Some(control_msg) = context.control_rx.recv() => {
                        if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                            state_helpers::emit_stopped(&context.state_tx, &node_name, "shutdown");
                            tracing::info!("AudioMixerNode shutting down (shutdown requested)");
                            return Ok(());
                        }
                    }

                    // Timeout-based mixing with silence, even when no new packets arrive.
                    () = async {
                        if let (Some(timeout), Some(start)) = (sync_timeout, waiting_since) {
                            let elapsed = start.elapsed();
                            if let Some(remaining) = timeout.checked_sub(elapsed) {
                                tokio::time::sleep(remaining).await;
                            }
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        let cold_start_complete = slots.iter().all(|s| s.has_sent);
                        if !cold_start_complete || waiting_since.is_none() {
                            continue;
                        }

                        let missing_idxs: Vec<usize> = slots
                            .iter()
                            .enumerate()
                            .filter(|(_idx, s)| !s.slow && s.frame.is_none())
                            .map(|(idx, _)| idx)
                            .collect();
                        if missing_idxs.is_empty() {
                            continue;
                        }

                        let missing_names: Vec<&str> =
                            missing_idxs.iter().map(|idx| slots[*idx].name.as_ref()).collect();
                        tracing::warn!(
                            "Mixer: timeout waiting for pins {:?}, mixing with silence",
                            missing_names
                        );

                        if !has_warned_slow {
                            state_helpers::emit_degraded(
                                &context.state_tx,
                                &node_name,
                                "slow_input_timeout"
                            );
                            has_warned_slow = true;
                        }

                        for idx in missing_idxs {
                            slots[idx].slow = true;
                        }
                        waiting_since = None;

                        if let Err(e) = self.mix_and_send(
                            &mut slots,
                            &mut mix_frames,
                            &mut context.output_sender,
                            max_output_channels_seen,
                            true,
                        ).await {
                            tracing::debug!("Output channel closed: {}", e);
                            return Ok(());
                        }

                        mixed_frame_count += 1;
                        if mixed_frame_count <= 5 || mixed_frame_count.is_multiple_of(50) {
                            tracing::trace!(
                                "[MIX_TRACE] Sent mixed frame #{} (with silence for slow pins)",
                                mixed_frame_count
                            );
                        }
                    }

                    // Receive from any input (we can't select! over a dynamic list of receivers)
                    result = Self::recv_from_any(&mut slots, &mut round_robin_idx, cancellation_token.as_ref()) => {
                        match result {
                            RecvResult::Audio(slot_idx, frame) => {
                                if slot_idx >= slots.len() {
                                    continue;
                                }

                                // Track maximum output channels; never decreases.
                                max_output_channels_seen = max_output_channels_seen.max(frame.channels);

                                let was_cold_start = !slots[slot_idx].has_sent;
                                slots[slot_idx].has_sent = true;

                                // Check if cold start is now complete for ALL pins.
                                let cold_start_complete = slots.iter().all(|s| s.has_sent);
                                if was_cold_start && cold_start_complete {
                                    tracing::trace!("[MIX_TRACE] Cold start complete - starting to mix");
                                }

                                let had_no_frames = slots.iter().all(|s| s.frame.is_none());
                                let expected_count = slots.iter().filter(|s| !s.slow).count();
                                if had_no_frames && expected_count > 1 {
                                    waiting_since = Some(std::time::Instant::now());
                                    has_warned_slow = false;
                                }

                                // Keep the latest frame per pin.
                                slots[slot_idx].frame = Some(frame);

                                let ready_to_mix = slots.iter().all(|s| s.slow || s.frame.is_some());
                                if ready_to_mix {
                                    waiting_since = None;

                                    let recovered_names: Vec<&str> = slots
                                        .iter()
                                        .filter(|s| s.slow && s.frame.is_some())
                                        .map(|s| s.name.as_ref())
                                        .collect();
                                    if !recovered_names.is_empty() {
                                        tracing::info!(
                                            "Mixer: Pins recovered from slow state: {:?}",
                                            recovered_names
                                        );
                                        for s in &mut slots {
                                            if s.slow && s.frame.is_some() {
                                                s.slow = false;
                                            }
                                        }
                                        if slots.iter().all(|s| !s.slow) && has_warned_slow {
                                            state_helpers::emit_running(&context.state_tx, &node_name);
                                            has_warned_slow = false;
                                        }
                                    }

                                    if let Err(e) = self.mix_and_send(
                                        &mut slots,
                                        &mut mix_frames,
                                        &mut context.output_sender,
                                        max_output_channels_seen,
                                        false,
                                    ).await {
                                        tracing::debug!("Output channel closed: {}", e);
                                        return Ok(());
                                    }

                                    mixed_frame_count += 1;
                                    if mixed_frame_count <= 5 || mixed_frame_count.is_multiple_of(50) {
                                        tracing::trace!("[MIX_TRACE] Sent mixed frame #{}", mixed_frame_count);
                                    }
                                } else if let Some(timeout) = sync_timeout {
                                    let cold_start_complete = slots.iter().all(|s| s.has_sent);
                                    if cold_start_complete {
                                        if let Some(start) = waiting_since {
                                            if start.elapsed() >= timeout {
                                                let missing_names: Vec<&str> = slots
                                                    .iter()
                                                    .filter(|s| !s.slow && s.frame.is_none())
                                                    .map(|s| s.name.as_ref())
                                                    .collect();

                                                if !missing_names.is_empty() {
                                                    if !has_warned_slow {
                                                        tracing::warn!(
                                                            "Mixer sync timeout ({}ms) expired. Missing frames from: {:?}. \
                                                             Marking as slow and will continue mixing without waiting.",
                                                            timeout.as_millis(),
                                                            missing_names
                                                        );
                                                        state_helpers::emit_degraded(
                                                            &context.state_tx,
                                                            &node_name,
                                                            "slow_input_timeout"
                                                        );
                                                        has_warned_slow = true;
                                                    }

                                                    for s in &mut slots {
                                                        if !s.slow && s.frame.is_none() {
                                                            s.slow = true;
                                                        }
                                                    }

                                                    waiting_since = None;
                                                    if let Err(e) = self.mix_and_send(
                                                        &mut slots,
                                                        &mut mix_frames,
                                                        &mut context.output_sender,
                                                        max_output_channels_seen,
                                                        true,
                                                    ).await {
                                                        tracing::debug!("Output channel closed: {}", e);
                                                        return Ok(());
                                                    }

                                                    mixed_frame_count += 1;
                                                    if mixed_frame_count <= 5 || mixed_frame_count.is_multiple_of(50) {
                                                        tracing::trace!(
                                                            "[MIX_TRACE] Sent mixed frame #{} (with silence for slow pins)",
                                                            mixed_frame_count
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        tracing::trace!(
                                            "Mixer waiting for cold start to complete. Pins that have sent: {}, Active pins: {}",
                                            slots.iter().filter(|s| s.has_sent).count(),
                                            slots.len()
                                        );
                                    }
                                }
                            }
                        RecvResult::PinEof(slot_idx) => {
                            if slot_idx >= slots.len() {
                                continue;
                            }

                            let pin_name = slots[slot_idx].name.as_ref().to_string();
                            tracing::info!(
                                "Mixer: Pin {} received EOF, removing from active set",
                                pin_name
                            );

                            slots.remove(slot_idx);
                            if round_robin_idx > slot_idx {
                                round_robin_idx = round_robin_idx.saturating_sub(1);
                            }
                            if slots.is_empty() {
                                round_robin_idx = 0;
                            } else {
                                round_robin_idx %= slots.len();
                            }
                            waiting_since = None;

                            if slots.iter().all(|s| !s.slow) && has_warned_slow {
                                state_helpers::emit_running(&context.state_tx, &node_name);
                                has_warned_slow = false;
                            }

                            // If we have frames buffered and remaining active pins, mix now.
                            if !slots.is_empty() && slots.iter().any(|s| s.frame.is_some()) {
                                if let Err(e) = self.mix_and_send(
                                    &mut slots,
                                    &mut mix_frames,
                                    &mut context.output_sender,
                                    max_output_channels_seen,
                                    false,
                                ).await {
                                    tracing::debug!("Output channel closed: {}", e);
                                    return Ok(());
                                }
                            }

                            if slots.is_empty() {
                                state_helpers::emit_stopped(
                                    &context.state_tx,
                                    &node_name,
                                    "all_inputs_closed"
                                );
                                tracing::info!("AudioMixerNode shutting down (all inputs closed)");
                                return Ok(());
                            }
                        }
                        RecvResult::OtherPacket => {
                            // Skip non-audio packets
                        }
                        RecvResult::AllClosed => {
                            // All inputs closed
                            state_helpers::emit_stopped(&context.state_tx, &node_name, "no_inputs");
                            tracing::info!("AudioMixerNode shutting down (no inputs)");
                            return Ok(());
                        }
                        RecvResult::Cancelled => {
                            // Cancellation requested
                            tracing::info!("AudioMixerNode cancelled");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// Mix buffered frames and send output
    async fn mix_and_send(
        &self,
        slots: &mut [InputSlot],
        mix_frames: &mut Vec<AudioFrame>,
        output_sender: &mut streamkit_core::OutputSender,
        max_output_channels_seen: u16,
        fill_silence: bool,
    ) -> Result<(), String> {
        let expected_count = slots.iter().filter(|s| !s.slow).count();
        let present_expected_count = slots.iter().filter(|s| !s.slow && s.frame.is_some()).count();

        mix_frames.clear();
        for slot in slots.iter_mut() {
            if let Some(frame) = slot.frame.take() {
                mix_frames.push(frame);
            }
        }

        if mix_frames.is_empty() {
            return Ok(());
        }

        // Determine output configuration.
        // Output channels never decrease across the lifetime of the node, to avoid downstream
        // format flips when a higher-channel input ends.
        let current_max = mix_frames.iter().map(|f| f.channels).max().unwrap_or(1);
        let output_channels = max_output_channels_seen.max(current_max).max(1);
        let sample_rate = mix_frames.first().map(|f| f.sample_rate).unwrap_or_default();

        // Calculate output frame size based on the longest input after channel conversion
        let max_samples_per_channel =
            mix_frames.iter().map(|f| f.samples.len() / f.channels as usize).max().unwrap_or(0);
        let output_size = max_samples_per_channel * output_channels as usize;
        let present_pins_count = mix_frames.len();

        // Optimization: if we have a frame that already matches the output shape, reuse it as the
        // output buffer and mix other frames into it (avoids allocating a fresh Vec per mix).
        // If samples are shared (Arc), `make_samples_mut()` will clone once (copy-on-write).
        let base_idx = mix_frames
            .iter()
            .enumerate()
            .filter(|(_idx, frame)| {
                frame.channels == output_channels && frame.samples.len() == output_size
            })
            .max_by_key(|(idx, frame)| (frame.has_unique_samples(), *idx))
            .map(|(idx, _)| idx);

        let mut output_frame = if let Some(base_idx) = base_idx {
            let mut base = mix_frames.swap_remove(base_idx);

            let output_samples = base.make_samples_mut();
            for frame_to_mix in &*mix_frames {
                Self::mix_frame_with_channel_conversion(
                    output_samples,
                    frame_to_mix,
                    output_channels,
                );
            }
            base
        } else {
            // Fallback: allocate a fresh output buffer and mix all frames into it.
            let mut mixed_samples = vec![0.0f32; output_size];
            for frame_to_mix in &*mix_frames {
                Self::mix_frame_with_channel_conversion(
                    &mut mixed_samples,
                    frame_to_mix,
                    output_channels,
                );
            }

            // Preserve metadata from the first frame (timestamp, duration, etc.)
            // Use take() instead of clone() to avoid copying - we're about to clear the buffer anyway
            let metadata = mix_frames.get_mut(0).and_then(|f| f.metadata.take());

            AudioFrame::with_metadata(sample_rate, output_channels, mixed_samples, metadata)
        };

        // If filling silence for missing pins, they contribute 0.0 (already initialized)
        if fill_silence {
            let missing_count = expected_count.saturating_sub(present_expected_count);
            if missing_count > 0 {
                tracing::debug!(
                    "Mixed {} present pins, {} missing pins treated as silence",
                    present_pins_count,
                    missing_count
                );
            }
        }

        // Ensure the output frame reports the selected output channel count (should already match,
        // but keep this explicit for future refactors).
        output_frame.channels = output_channels;

        output_sender.send("out", Packet::Audio(output_frame)).await.map_err(|e| e.to_string())?;

        mix_frames.clear();
        Ok(())
    }

    /// Mix a source frame into the output buffer, handling channel conversion
    /// Supports:
    /// - Mono (1ch) -> Stereo (2ch): Duplicate mono signal to both channels
    /// - Stereo (2ch) -> Stereo (2ch): Direct mixing
    /// - Other configurations: Basic channel mapping
    #[allow(clippy::needless_range_loop)]
    fn mix_frame_with_channel_conversion(
        output: &mut [f32],
        source: &AudioFrame,
        output_channels: u16,
    ) {
        let source_channels = source.channels;
        let samples_per_channel = source.samples.len() / source_channels as usize;
        let output_samples_per_channel = output.len() / output_channels as usize;

        // Use the minimum length to avoid out-of-bounds
        let mix_samples_per_channel = samples_per_channel.min(output_samples_per_channel);

        if source_channels == output_channels {
            // Same channel count: direct sample-wise mixing
            let mix_len = mix_samples_per_channel * output_channels as usize;
            for (out_sample, src_sample) in
                output.iter_mut().zip(source.samples.iter()).take(mix_len)
            {
                *out_sample += src_sample;
            }
        } else if source_channels == 1 && output_channels == 2 {
            // Mono to stereo: duplicate mono signal to both L and R channels
            for i in 0..mix_samples_per_channel {
                let mono_sample = source.samples[i];
                let out_idx = i * 2;
                output[out_idx] += mono_sample; // Left channel
                output[out_idx + 1] += mono_sample; // Right channel
            }
        } else if source_channels == 2 && output_channels == 1 {
            // Stereo to mono: average L and R channels
            for i in 0..mix_samples_per_channel {
                let left = source.samples[i * 2];
                let right = source.samples[i * 2 + 1];
                output[i] += (left + right) * 0.5;
            }
        } else {
            // Generic fallback: map channels cyclically
            tracing::warn!(
                "Mixing {} channels into {} channels using generic fallback",
                source_channels,
                output_channels
            );
            for i in 0..mix_samples_per_channel {
                for ch in 0..(output_channels as usize) {
                    let source_ch = ch % source_channels as usize;
                    let source_idx = i * source_channels as usize + source_ch;
                    let output_idx = i * output_channels as usize + ch;
                    output[output_idx] += source.samples[source_idx];
                }
            }
        }
    }

    /// Helper to receive from any input channel
    /// Returns a RecvResult indicating what was received
    ///
    /// Strategy:
    /// 1. First, do a non-blocking check of all receivers (fair round-robin starting from round_robin_idx)
    /// 2. If no data available, wait on the current round-robin receiver using proper async
    ///
    /// The try_recv pass provides fairness across inputs. The async wait on the current receiver
    /// is efficient (no busy-polling) and the outer select! loop will cycle back to check all
    /// receivers again after this returns.
    ///
    /// Performance: Avoids hashing by polling an indexed slice of receivers.
    async fn recv_from_any(
        slots: &mut [InputSlot],
        round_robin_idx: &mut usize,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> RecvResult {
        if slots.is_empty() {
            // No receivers, wait indefinitely (will be woken by pin management)
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            return RecvResult::AllClosed;
        }

        let num_receivers = slots.len();

        // Fair round-robin: start from last position to avoid starvation
        for i in 0..num_receivers {
            let idx = (*round_robin_idx + i) % num_receivers;
            match slots[idx].rx.try_recv() {
                Ok(Packet::Audio(frame)) => {
                    *round_robin_idx = (idx + 1) % num_receivers;
                    return RecvResult::Audio(idx, frame);
                },
                Ok(_other) => return RecvResult::OtherPacket,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {},
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return RecvResult::PinEof(idx);
                },
            }
        }

        // No data immediately available. Wait on the current round-robin receiver using proper async.
        // This is more efficient than busy-polling. The outer select! loop will cycle
        // back to check all receivers again after this returns.
        let wait_idx = *round_robin_idx % num_receivers;
        let rx = &mut slots[wait_idx].rx;

        if let Some(token) = cancellation_token {
            tokio::select! {
                biased;
                () = token.cancelled() => RecvResult::Cancelled,
                result = rx.recv() => {
                    match result {
                        Some(Packet::Audio(frame)) => RecvResult::Audio(wait_idx, frame),
                        Some(_other) => RecvResult::OtherPacket,
                        None => RecvResult::PinEof(wait_idx),
                    }
                }
            }
        } else {
            match rx.recv().await {
                Some(Packet::Audio(frame)) => RecvResult::Audio(wait_idx, frame),
                Some(_other) => RecvResult::OtherPacket,
                None => RecvResult::PinEof(wait_idx),
            }
        }
    }
}

#[derive(Clone)]
struct OutputMailbox {
    latest: Arc<Mutex<Option<AudioFrame>>>,
    notify: Arc<tokio::sync::Notify>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl OutputMailbox {
    fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            latest: Arc::new(Mutex::new(None)),
            notify: Arc::new(tokio::sync::Notify::new()),
            shutdown_tx,
            shutdown_rx,
        }
    }

    fn publish(&self, frame: AudioFrame) {
        if let Ok(mut guard) = self.latest.lock() {
            *guard = Some(frame);
        }
        self.notify.notify_one();
    }

    fn take_latest(&self) -> Option<AudioFrame> {
        self.latest.lock().ok().and_then(|mut g| g.take())
    }

    fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.notify.notify_waiters();
    }
}

struct InputRingBuffer {
    capacity: usize,
    queue: Mutex<VecDeque<AudioFrame>>,
}

impl InputRingBuffer {
    fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), queue: Mutex::new(VecDeque::new()) }
    }

    fn push(&self, frame: AudioFrame) {
        let Ok(mut guard) = self.queue.lock() else { return };
        if guard.len() >= self.capacity {
            let _ = guard.pop_front();
        }
        guard.push_back(frame);
    }

    fn pop(&self) -> Option<AudioFrame> {
        self.queue.lock().ok().and_then(|mut g| g.pop_front())
    }
}

enum AudioThreadCommand {
    AddInput { name: Arc<str>, ring: Arc<InputRingBuffer> },
    RemoveInput { name: Arc<str> },
    Shutdown,
}

enum InputEvent {
    Eof(Arc<str>),
}

struct ClockedThreadConfig {
    node_name: String,
    sample_rate: u32,
    frame_samples_per_channel: usize,
    tick_duration: std::time::Duration,
    generate_silence: bool,
    sync_timeout: Option<std::time::Duration>,
    audio_pool: Option<Arc<AudioFramePool>>,
    state_tx: tokio::sync::mpsc::Sender<streamkit_core::state::NodeStateUpdate>,
    output_mailbox: Arc<OutputMailbox>,
    cmd_rx: std::sync::mpsc::Receiver<AudioThreadCommand>,
    stop_flag: Arc<AtomicBool>,
}

struct ClockedInputState {
    name: Arc<str>,
    ring: Arc<InputRingBuffer>,
    slow: bool,
    missing_since: Option<std::time::Instant>,
    has_ever_sent: bool,
}

fn run_clocked_audio_thread(config: &ClockedThreadConfig) {
    let mut inputs: Vec<ClockedInputState> = Vec::new();

    let mut max_output_channels_seen: u16 = 0;
    let mut has_warned_slow = false;
    let mut next_tick = std::time::Instant::now() + config.tick_duration;

    let tick_us = (config.frame_samples_per_channel as u64).saturating_mul(1_000_000)
        / u64::from(config.sample_rate.max(1));

    loop {
        if config.stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let now = std::time::Instant::now();
        let timeout = next_tick.saturating_duration_since(now);

        match config.cmd_rx.recv_timeout(timeout) {
            Ok(cmd) => match cmd {
                AudioThreadCommand::AddInput { name, ring } => {
                    if inputs.iter().any(|i| i.name == name) {
                        continue;
                    }
                    inputs.push(ClockedInputState {
                        name,
                        ring,
                        slow: false,
                        missing_since: None,
                        has_ever_sent: false,
                    });
                },
                AudioThreadCommand::RemoveInput { name } => {
                    inputs.retain(|i| i.name != name);
                    if inputs.is_empty() && has_warned_slow {
                        state_helpers::emit_running(&config.state_tx, &config.node_name);
                        has_warned_slow = false;
                    }
                },
                AudioThreadCommand::Shutdown => break,
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Tick.
                next_tick += config.tick_duration;

                if inputs.is_empty() {
                    continue;
                }

                // Drain any queued commands quickly.
                while let Ok(cmd) = config.cmd_rx.try_recv() {
                    match cmd {
                        AudioThreadCommand::AddInput { name, ring } => {
                            if inputs.iter().any(|i| i.name == name) {
                                continue;
                            }
                            inputs.push(ClockedInputState {
                                name,
                                ring,
                                slow: false,
                                missing_since: None,
                                has_ever_sent: false,
                            });
                        },
                        AudioThreadCommand::RemoveInput { name } => {
                            inputs.retain(|i| i.name != name);
                        },
                        AudioThreadCommand::Shutdown => {
                            return;
                        },
                    }
                }

                let mut frames: Vec<AudioFrame> = Vec::new();
                let mut any_input_had_frame = false;
                let sync_timeout = config.sync_timeout;

                for input in &mut inputs {
                    let frame = input.ring.pop();
                    if let Some(frame) = frame {
                        if frame.sample_rate != config.sample_rate {
                            tracing::warn!(
                                "Clocked mixer input '{}' sample_rate mismatch: got {}, expected {} (dropping frame)",
                                input.name,
                                frame.sample_rate,
                                config.sample_rate
                            );
                            continue;
                        }

                        max_output_channels_seen = max_output_channels_seen.max(frame.channels);
                        input.has_ever_sent = true;
                        input.missing_since = None;

                        if input.slow {
                            input.slow = false;
                        }

                        any_input_had_frame = true;
                        frames.push(frame);
                    } else {
                        // Missing this tick.
                        if input.missing_since.is_none() {
                            input.missing_since = Some(std::time::Instant::now());
                        }

                        if !input.slow {
                            if let (Some(since), Some(timeout)) =
                                (input.missing_since, sync_timeout)
                            {
                                if since.elapsed() >= timeout {
                                    input.slow = true;
                                }
                            }
                        }
                    }
                }

                let any_slow = inputs.iter().any(|i| i.slow);
                if any_slow && !has_warned_slow {
                    state_helpers::emit_degraded(
                        &config.state_tx,
                        &config.node_name,
                        "slow_input_timeout",
                    );
                    has_warned_slow = true;
                } else if !any_slow && has_warned_slow {
                    state_helpers::emit_running(&config.state_tx, &config.node_name);
                    has_warned_slow = false;
                }

                if !any_input_had_frame && !config.generate_silence {
                    continue;
                }

                // If we've never observed channels yet, we can't size the output buffer.
                if max_output_channels_seen == 0 && frames.is_empty() {
                    continue;
                }

                let output_channels = max_output_channels_seen
                    .max(frames.iter().map(|f| f.channels).max().unwrap_or(1))
                    .max(1);

                let metadata =
                    frames.get_mut(0).and_then(|f| f.metadata.take()).or(Some(PacketMetadata {
                        timestamp_us: None,
                        duration_us: Some(tick_us),
                        sequence: None,
                    }));

                let output_frame = mix_clocked_frames(
                    &mut frames,
                    config.sample_rate,
                    output_channels,
                    config.frame_samples_per_channel,
                    metadata,
                    config.audio_pool.as_deref(),
                );

                config.output_mailbox.publish(output_frame);
            },
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn mix_clocked_frames(
    frames: &mut Vec<AudioFrame>,
    sample_rate: u32,
    output_channels: u16,
    frame_samples_per_channel: usize,
    metadata: Option<PacketMetadata>,
    audio_pool: Option<&AudioFramePool>,
) -> AudioFrame {
    let output_size = frame_samples_per_channel * output_channels as usize;

    // If we have a frame that matches the output shape, reuse it as the output buffer.
    let base_idx = frames
        .iter()
        .enumerate()
        .filter(|(_idx, frame)| {
            frame.channels == output_channels && frame.samples.len() == output_size
        })
        .max_by_key(|(idx, frame)| (frame.has_unique_samples(), *idx))
        .map(|(idx, _)| idx);

    let mut output_frame = if let Some(base_idx) = base_idx {
        let mut base = frames.swap_remove(base_idx);
        let output_samples = base.make_samples_mut();
        for frame_to_mix in &*frames {
            AudioMixerNode::mix_frame_with_channel_conversion(
                output_samples,
                frame_to_mix,
                output_channels,
            );
        }
        base.metadata = metadata;
        base
    } else {
        let mut mixed_samples = audio_pool.map_or_else(
            || streamkit_core::PooledSamples::from_vec(vec![0.0f32; output_size]),
            |pool| {
                let mut pooled = pool.get(output_size);
                pooled.as_mut_slice().fill(0.0);
                pooled
            },
        );

        for frame_to_mix in &*frames {
            AudioMixerNode::mix_frame_with_channel_conversion(
                mixed_samples.as_mut_slice(),
                frame_to_mix,
                output_channels,
            );
        }

        AudioFrame::from_pooled(sample_rate, output_channels, mixed_samples, metadata)
    };

    output_frame.sample_rate = sample_rate;
    output_frame.channels = output_channels;
    output_frame
}

async fn run_input_drainer(
    name: Arc<str>,
    mut rx: mpsc::Receiver<Packet>,
    ring: Arc<InputRingBuffer>,
    cancellation_token: Option<tokio_util::sync::CancellationToken>,
    input_event_tx: mpsc::Sender<InputEvent>,
    expected_sample_rate: u32,
) {
    loop {
        let packet = if let Some(token) = &cancellation_token {
            tokio::select! {
                () = token.cancelled() => None,
                packet = rx.recv() => packet,
            }
        } else {
            rx.recv().await
        };

        let Some(packet) = packet else {
            let _ = input_event_tx.send(InputEvent::Eof(name)).await;
            return;
        };

        if let Packet::Audio(frame) = packet {
            if frame.sample_rate != expected_sample_rate {
                tracing::warn!(
                    "Clocked mixer input '{}' sample_rate mismatch: got {}, expected {} (dropping frame)",
                    name,
                    frame.sample_rate,
                    expected_sample_rate
                );
                continue;
            }
            ring.push(frame);
        }

        // Drain bursty backlogs quickly.
        loop {
            match rx.try_recv() {
                Ok(Packet::Audio(frame)) => {
                    if frame.sample_rate == expected_sample_rate {
                        ring.push(frame);
                    }
                },
                Ok(_other) => {},
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    let _ = input_event_tx.send(InputEvent::Eof(name)).await;
                    return;
                },
            }
        }
    }
}

fn start_clocked_input_drainer(
    audio_cmd_tx: &std::sync::mpsc::Sender<AudioThreadCommand>,
    input_event_tx: &mpsc::Sender<InputEvent>,
    cancellation_token: Option<tokio_util::sync::CancellationToken>,
    jitter_buffer_frames: usize,
    expected_sample_rate: u32,
    pin_name: String,
    rx: mpsc::Receiver<Packet>,
) -> (Arc<str>, tokio::task::JoinHandle<()>) {
    let name: Arc<str> = Arc::from(pin_name);
    let ring = Arc::new(InputRingBuffer::new(jitter_buffer_frames));
    let _ =
        audio_cmd_tx.send(AudioThreadCommand::AddInput { name: name.clone(), ring: ring.clone() });

    let input_event_tx = input_event_tx.clone();
    let name_for_task = name.clone();
    let handle = tokio::spawn(async move {
        run_input_drainer(
            name_for_task,
            rx,
            ring,
            cancellation_token,
            input_event_tx,
            expected_sample_rate,
        )
        .await;
    });

    (name, handle)
}

/// Result from receiving from any input channel
enum RecvResult {
    /// Received an audio frame from the specified slot index
    Audio(usize, AudioFrame),
    /// A pin received EOF (channel closed)
    PinEof(usize),
    /// Received a non-audio packet (to be skipped)
    OtherPacket,
    /// All input channels are closed
    AllClosed,
    /// Cancellation was requested
    Cancelled,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::uninlined_format_args, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped,
        create_test_audio_packet, create_test_context, extract_audio_data,
    };
    use std::collections::HashMap;
    use streamkit_core::state::NodeStateUpdate;
    use tokio::sync::mpsc;

    async fn assert_state_stopped_eventually(
        state_rx: &mut mpsc::Receiver<NodeStateUpdate>,
        timeout: std::time::Duration,
    ) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let update = tokio::time::timeout(remaining, state_rx.recv())
                .await
                .ok()
                .flatten()
                .expect("Timeout waiting for Stopped state");

            if matches!(update.state, streamkit_core::NodeState::Stopped { .. }) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn test_mixer_two_inputs() {
        // Create two input channels
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create mixer - now always dynamic mode
        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send packets to both inputs (10 samples per channel, 2 channels = 20 total samples)
        // Input 1: all samples = 0.5
        // Input 2: all samples = 0.3
        // Expected output: all samples = 0.8
        let packet1 = create_test_audio_packet(48000, 2, 10, 0.5);
        let packet2 = create_test_audio_packet(48000, 2, 10, 0.3);

        input1_tx.send(packet1).await.unwrap();
        input2_tx.send(packet2).await.unwrap();

        // Give time for mixing to occur
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify mixed output - should have one packet
        let output_packets = mock_sender.get_packets_for_pin("out").await;

        // Close inputs after checking packets to allow clean shutdown
        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1, "Expected 1 mixed packet");

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");
        assert_eq!(audio_data.len(), 20); // 10 samples * 2 channels

        // Verify mixing: 0.5 + 0.3 = 0.8
        for &sample in audio_data {
            assert!((sample - 0.8).abs() < 0.001, "Expected ~0.8, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_continues_after_eof_with_sticky_channels() {
        let (stereo_tx, stereo_rx) = mpsc::channel(10);
        let (mono_tx, mono_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), stereo_rx);
        inputs.insert("in_1".to_string(), mono_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Force pre-created pins so stateless wiring matches typical usage.
        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            num_inputs: Some(2),
            clocked: None,
        });

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // First mix: stereo + mono -> stereo output.
        stereo_tx.send(create_test_audio_packet(48000, 2, 10, 0.5)).await.unwrap();
        mono_tx.send(create_test_audio_packet(48000, 1, 10, 0.3)).await.unwrap();

        let (_node, pin, packet) = mock_sender
            .recv_timeout(std::time::Duration::from_secs(2))
            .await
            .expect("Expected a mixed packet");
        assert_eq!(pin, "out");
        let Packet::Audio(frame) = packet else { panic!("Expected audio packet") };
        assert_eq!(frame.channels, 2);
        assert!((frame.samples[0] - 0.8).abs() < 0.001);

        // End the stereo input (e.g., music track finished); mixing should continue.
        drop(stereo_tx);

        // Next mix: mono only, but output channels should remain sticky at 2 (mono upmixed).
        mono_tx.send(create_test_audio_packet(48000, 1, 10, 0.25)).await.unwrap();

        let (_node, pin, packet) = mock_sender
            .recv_timeout(std::time::Duration::from_secs(2))
            .await
            .expect("Expected a mixed packet after EOF");
        assert_eq!(pin, "out");
        let Packet::Audio(frame) = packet else { panic!("Expected audio packet") };
        assert_eq!(frame.channels, 2);
        assert!((frame.samples[0] - 0.25).abs() < 0.001);
        assert!((frame.samples[1] - 0.25).abs() < 0.001);

        drop(mono_tx);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_mixer_three_inputs() {
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);
        let (input3_tx, input3_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);
        inputs.insert("in_2".to_string(), input3_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send packets with different values
        // Input 1: 0.1, Input 2: 0.2, Input 3: 0.3
        // Expected: 0.6
        input1_tx.send(create_test_audio_packet(48000, 2, 10, 0.1)).await.unwrap();
        input2_tx.send(create_test_audio_packet(48000, 2, 10, 0.2)).await.unwrap();
        input3_tx.send(create_test_audio_packet(48000, 2, 10, 0.3)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let output_packets = mock_sender.get_packets_for_pin("out").await;

        drop(input1_tx);
        drop(input2_tx);
        drop(input3_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1);

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");

        for &sample in audio_data {
            assert!((sample - 0.6).abs() < 0.001, "Expected ~0.6, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_basic_mixing_math() {
        // Simple test to verify the mixing math is correct
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Test: 0.1 + 0.2 = 0.3
        input1_tx.send(create_test_audio_packet(48000, 2, 10, 0.1)).await.unwrap();
        input2_tx.send(create_test_audio_packet(48000, 2, 10, 0.2)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let output_packets = mock_sender.get_packets_for_pin("out").await;

        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1);

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");
        for &sample in audio_data {
            assert!((sample - 0.3).abs() < 0.001, "Expected ~0.3, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_silence_mixing() {
        // Test mixing silence (0.0) with audio
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Input 1: silence, Input 2: 1.0
        // Expected: 1.0
        input1_tx.send(create_test_audio_packet(48000, 2, 10, 0.0)).await.unwrap();
        input2_tx.send(create_test_audio_packet(48000, 2, 10, 1.0)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let output_packets = mock_sender.get_packets_for_pin("out").await;

        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1);
        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");

        for &sample in audio_data {
            assert!((sample - 1.0).abs() < 0.001, "Expected ~1.0, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_negative_values() {
        // Test mixing with negative values (phase inversion)
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Input 1: 0.5, Input 2: -0.3
        // Expected: 0.2
        input1_tx.send(create_test_audio_packet(48000, 2, 10, 0.5)).await.unwrap();
        input2_tx.send(create_test_audio_packet(48000, 2, 10, -0.3)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let output_packets = mock_sender.get_packets_for_pin("out").await;

        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1);
        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");

        for &sample in audio_data {
            assert!((sample - 0.2).abs() < 0.001, "Expected ~0.2, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_single_input() {
        // Edge case: mixer with just one input should pass through
        let (input1_tx, input1_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input1_tx.send(create_test_audio_packet(48000, 2, 10, 0.75)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let output_packets = mock_sender.get_packets_for_pin("out").await;

        drop(input1_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        assert_eq!(output_packets.len(), 1);

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");

        for &sample in audio_data {
            assert!((sample - 0.75).abs() < 0.001, "Expected ~0.75, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_mixer_input_pins() {
        // Verify mixer starts with no pins in dynamic mode
        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let pins = node.input_pins();

        // Dynamic mode starts with 0 pins - they are added at runtime
        assert_eq!(pins.len(), 0);
    }

    #[tokio::test]
    async fn test_mixer_output_pins() {
        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(100),
            ..Default::default()
        });
        let pins = node.output_pins();

        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "out");
    }

    #[tokio::test]
    async fn test_clocked_mixer_two_inputs() {
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(50),
            clocked: Some(ClockedMixerConfig {
                sample_rate: 48_000,
                frame_samples_per_channel: 10,
                jitter_buffer_frames: 2,
                generate_silence: false,
            }),
            ..Default::default()
        });

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input1_tx.send(create_test_audio_packet(48_000, 2, 10, 0.5)).await.unwrap();
        input2_tx.send(create_test_audio_packet(48_000, 2, 10, 0.3)).await.unwrap();

        let (_node, pin, packet) = mock_sender
            .recv_timeout(std::time::Duration::from_secs(2))
            .await
            .expect("Expected a mixed packet");
        assert_eq!(pin, "out");

        let Packet::Audio(frame) = packet else { panic!("Expected audio packet") };
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples.len(), 20);
        for &sample in frame.samples.as_slice() {
            assert!((sample - 0.8).abs() < 0.001, "Expected ~0.8, got {}", sample);
        }

        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert_state_stopped_eventually(&mut state_rx, std::time::Duration::from_secs(2)).await;
        node_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_clocked_mixer_missing_input_mixes_silence() {
        let (input1_tx, input1_rx) = mpsc::channel(10);
        let (input2_tx, input2_rx) = mpsc::channel(10);

        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input1_rx);
        inputs.insert("in_1".to_string(), input2_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioMixerNode::new(AudioMixerConfig {
            sync_timeout_ms: Some(50),
            clocked: Some(ClockedMixerConfig {
                sample_rate: 48_000,
                frame_samples_per_channel: 10,
                jitter_buffer_frames: 2,
                generate_silence: false,
            }),
            ..Default::default()
        });

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input1_tx.send(create_test_audio_packet(48_000, 2, 10, 0.75)).await.unwrap();

        let (_node, pin, packet) = mock_sender
            .recv_timeout(std::time::Duration::from_secs(2))
            .await
            .expect("Expected a mixed packet");
        assert_eq!(pin, "out");

        let Packet::Audio(frame) = packet else { panic!("Expected audio packet") };
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples.len(), 20);
        for &sample in frame.samples.as_slice() {
            assert!((sample - 0.75).abs() < 0.001, "Expected ~0.75, got {}", sample);
        }

        drop(input1_tx);
        drop(input2_tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert_state_stopped_eventually(&mut state_rx, std::time::Duration::from_secs(2)).await;
        node_handle.await.unwrap().unwrap();
    }
}
