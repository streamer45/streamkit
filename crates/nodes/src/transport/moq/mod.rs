// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ (Media over QUIC) transport nodes
//!
//! This module provides nodes for working with MoQ streams:
//! - `moq_pull`: Client that subscribes to broadcasts from a MoQ server
//! - `moq_push`: Client that publishes packets to a MoQ server
//! - `moq_peer`: Bidirectional server that accepts WebTransport connections

#![cfg(feature = "moq")]

mod constants;
mod peer;
mod pull;
mod push;

use std::sync::OnceLock;

// Re-export public types
pub use peer::{MoqPeerConfig, MoqPeerNode};
pub use pull::{MoqPullConfig, MoqPullNode};
pub use push::{MoqPushConfig, MoqPushNode};

use schemars::schema_for;
use streamkit_core::{
    config_helpers, registry::StaticPins, NodeRegistry, ProcessorNode, StreamKitError,
};

static SHARED_INSECURE_CLIENT: OnceLock<Result<moq_native::Client, String>> = OnceLock::new();

fn shared_insecure_client() -> Result<moq_native::Client, StreamKitError> {
    let client = SHARED_INSECURE_CLIENT.get_or_init(|| {
        let mut client_config = moq_native::ClientConfig::default();
        // For local dev/test we disable verification; moq-native still loads native roots, so
        // caching the initialized client avoids repeated expensive cert parsing.
        client_config.tls.disable_verify = Some(true);
        client_config.init().map_err(|e| format!("Failed to create MoQ client: {e}"))
    });

    match client {
        Ok(client) => Ok(client.clone()),
        Err(message) => Err(StreamKitError::Runtime(message.clone())),
    }
}

/// Registers the MoQ transport nodes.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_moq_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "moq")]
    {
        let default_moq_pull = MoqPullNode::new(MoqPullConfig::default());
        registry.register_static_with_description(
            "transport::moq::subscriber",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPullNode::new(config)))
            },
            serde_json::to_value(schema_for!(MoqPullConfig))
                .expect("MoqPullConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_moq_pull.input_pins(),
                outputs: default_moq_pull.output_pins(),
            },
            vec!["transport".to_string(), "moq".to_string(), "dynamic".to_string()],
            false,
            "Subscribes to a Media over QUIC (MoQ) broadcast. \
             Receives Opus audio from a remote publisher over WebTransport.",
        );

        let default_moq_push = MoqPushNode::new(MoqPushConfig::default());
        registry.register_static_with_description(
            "transport::moq::publisher",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPushNode::new(config)))
            },
            serde_json::to_value(schema_for!(MoqPushConfig))
                .expect("MoqPushConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_moq_push.input_pins(),
                outputs: default_moq_push.output_pins(),
            },
            vec!["transport".to_string(), "moq".to_string(), "dynamic".to_string()],
            false,
            "Publishes audio to a Media over QUIC (MoQ) broadcast. \
             Sends Opus audio to subscribers over WebTransport.",
        );

        let default_moq_peer = MoqPeerNode::new(MoqPeerConfig::default());
        registry.register_static_with_description(
            "transport::moq::peer",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPeerNode::new(config)))
            },
            serde_json::to_value(schema_for!(MoqPeerConfig))
                .expect("MoqPeerConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_moq_peer.input_pins(),
                outputs: default_moq_peer.output_pins(),
            },
            vec![
                "transport".to_string(),
                "moq".to_string(),
                "bidirectional".to_string(),
                "dynamic".to_string(),
            ],
            true, // This is a bidirectional node
            "Bidirectional MoQ peer for real-time audio communication. \
             Acts as both publisher and subscriber over a single WebTransport connection.",
        );
    }
}
