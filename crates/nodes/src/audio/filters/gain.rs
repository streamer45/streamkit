// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::types::{AudioFormat, Packet, PacketType, SampleFormat};
use streamkit_core::{
    packet_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::select;

fn gain_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "number",
        "default": 1.0,
        "minimum": 0.0,
        "maximum": 4.0,
        "tunable": true,
        "description": "Linear gain multiplier. 0.0 = mute, 1.0 = unity (no change), 2.0 = +6dB, 4.0 = +12dB. Range: 0.0 to 4.0"
    })
}

#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioGainConfig {
    #[schemars(schema_with = "gain_schema")]
    pub gain: f32,
}

impl Default for AudioGainConfig {
    fn default() -> Self {
        Self { gain: 1.0 } // Default to no volume change
    }
}

impl AudioGainConfig {
    /// # Errors
    /// Returns `Err` if gain is non-finite or outside `[0.0, 4.0]`.
    pub fn validate(&self) -> Result<(), String> {
        const MIN_GAIN: f32 = 0.0;
        const MAX_GAIN: f32 = 4.0;

        if !self.gain.is_finite() {
            return Err(format!("Gain must be a finite number, got: {}", self.gain));
        }

        if self.gain < MIN_GAIN || self.gain > MAX_GAIN {
            return Err(format!(
                "Gain must be between {} and {}, got: {}",
                MIN_GAIN, MAX_GAIN, self.gain
            ));
        }

        Ok(())
    }
}

pub struct AudioGainNode {
    config: AudioGainConfig,
}

impl AudioGainNode {
    /// # Errors
    /// Returns `Err` if config validation fails.
    pub fn new(config: AudioGainConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for AudioGainNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::RawAudio(AudioFormat {
                sample_rate: 0,
                channels: 0,
                sample_format: SampleFormat::F32,
            })],
            cardinality: PinCardinality::One,
        }]
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

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;

        tracing::info!("AudioGainNode starting with gain: {}", self.config.gain);

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut control_rx = context.control_rx;
        let mut packet_count = 0;

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        loop {
            select! {
                maybe_packet = input_rx.recv() => {
                    if let Some(first_packet) = maybe_packet {
                        let packet_batch = packet_helpers::batch_packets_greedy(
                            first_packet,
                            &mut input_rx,
                            context.batch_size,
                        );

                        for mut packet in packet_batch {
                             packet_count += 1;
                             stats_tracker.received();

                            while let Ok(ctrl_msg) = control_rx.try_recv() {
                                match ctrl_msg {
                                    NodeControlMessage::UpdateParams(params) => {
                                        match serde_json::from_value::<AudioGainConfig>(params) {
                                            Ok(new_config) => {
                                                match new_config.validate() {
                                                    Ok(()) => {
                                                        tracing::info!(old = self.config.gain, new = new_config.gain, "Updating volume gain");
                                                        self.config = new_config;
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("Rejected invalid gain parameter: {}", e);
                                                        stats_tracker.errored();
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!("Failed to deserialize params for volume_adjust: {}", e);
                                                stats_tracker.errored();
                                            }
                                        }
                                    },
                                    NodeControlMessage::Start => {},
                                    NodeControlMessage::Shutdown => {
                                        tracing::info!("AudioGainNode received shutdown signal");
                                        return Ok(());
                                    },
                                }
                            }

                            if let Packet::Audio(ref mut frame) = packet {
                                for sample in frame.make_samples_mut() {
                                    *sample *= self.config.gain;
                                }
                            }
                            if context.output_sender.send("out", packet).await.is_err() {
                                tracing::debug!("Output channel closed, stopping node");
                                state_helpers::emit_stopped(&context.state_tx, &node_name, "output_closed");
                                return Ok(());
                            }
                            stats_tracker.sent();
                        }

                        stats_tracker.maybe_send();
                    } else {
                        tracing::info!("AudioGainNode input stream closed after {} packets", packet_count);
                        break;
                    }
                }
            }
        }

        while let Ok(ctrl_msg) = control_rx.try_recv() {
            if matches!(ctrl_msg, NodeControlMessage::Shutdown) {
                tracing::debug!("AudioGainNode received shutdown signal after input closed");
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("AudioGainNode shutting down.");
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::uninlined_format_args,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::used_underscore_binding
)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped,
        create_test_audio_packet, create_test_context, extract_audio_data,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_gain_happy_path() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 2.0 }).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let test_packet = create_test_audio_packet(48000, 2, 50, 0.5);
        input_tx.send(test_packet).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1, "Expected 1 output packet");

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio packet");
        assert_eq!(audio_data.len(), 100); // 50 samples * 2 channels

        for &sample in audio_data {
            assert!((sample - 1.0).abs() < 0.001, "Expected sample value ~1.0, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_gain_multiple_packets() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 0.5 }).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        for i in 0..3 {
            let value = 1.0 + i as f32;
            let packet = create_test_audio_packet(48000, 2, 10, value);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 3, "Expected 3 output packets");

        for (i, packet) in output_packets.iter().enumerate() {
            let audio_data = extract_audio_data(packet).expect("Should be audio packet");
            let expected_value = (1.0 + i as f32) * 0.5;
            for &sample in audio_data {
                assert!(
                    (sample - expected_value).abs() < 0.001,
                    "Packet {}: expected ~{}, got {}",
                    i,
                    expected_value,
                    sample
                );
            }
        }
    }

    #[tokio::test]
    async fn test_gain_parameter_update() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 1.0 }).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let packet1 = create_test_audio_packet(48000, 2, 10, 0.5);
        input_tx.send(packet1).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let packet2 = create_test_audio_packet(48000, 2, 10, 0.8);
        input_tx.send(packet2).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 2);
    }

    #[tokio::test]
    async fn test_gain_zero() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 0.0 }).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let packet = create_test_audio_packet(48000, 2, 10, 1.0);
        input_tx.send(packet).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        let audio_data = extract_audio_data(&output_packets[0]).unwrap();
        for &sample in audio_data {
            assert_eq!(sample, 0.0, "Expected silence");
        }
    }

    #[tokio::test]
    async fn test_gain_at_max() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 4.0 }).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let packet = create_test_audio_packet(48000, 2, 10, 0.5);
        input_tx.send(packet).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        let audio_data = extract_audio_data(&output_packets[0]).unwrap();

        for &sample in audio_data {
            assert!((sample - 2.0).abs() < 0.001, "Expected 2.0, got {}", sample);
        }
    }

    #[tokio::test]
    async fn test_gain_empty_input() {
        let (_input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = AudioGainNode::new(AudioGainConfig { gain: 1.0 }).unwrap();

        drop(_input_tx);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;
        assert_state_stopped(&mut state_rx).await;

        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 0, "No output expected");
    }

    #[test]
    fn test_gain_validation_valid_range() {
        assert!(AudioGainConfig { gain: 0.0 }.validate().is_ok());
        assert!(AudioGainConfig { gain: 1.0 }.validate().is_ok());
        assert!(AudioGainConfig { gain: 2.0 }.validate().is_ok());
        assert!(AudioGainConfig { gain: 4.0 }.validate().is_ok());
        assert!(AudioGainConfig { gain: 0.5 }.validate().is_ok());
        assert!(AudioGainConfig { gain: 3.5 }.validate().is_ok());
    }

    #[test]
    fn test_gain_validation_out_of_range() {
        let result = AudioGainConfig { gain: 4.1 }.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be between"));

        let result = AudioGainConfig { gain: -0.1 }.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be between"));

        let result = AudioGainConfig { gain: 100.0 }.validate();
        assert!(result.is_err());

        let result = AudioGainConfig { gain: -10.0 }.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_gain_validation_special_values() {
        let result = AudioGainConfig { gain: f32::NAN }.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite number"));

        let result = AudioGainConfig { gain: f32::INFINITY }.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite number"));

        let result = AudioGainConfig { gain: f32::NEG_INFINITY }.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite number"));
    }

    #[test]
    fn test_gain_constructor_validation() {
        assert!(AudioGainNode::new(AudioGainConfig { gain: 1.0 }).is_ok());
        assert!(AudioGainNode::new(AudioGainConfig { gain: 100.0 }).is_err());
        assert!(AudioGainNode::new(AudioGainConfig { gain: f32::NAN }).is_err());
    }
}
