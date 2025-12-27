// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for container nodes (OGG, WAV, WebM)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]

use super::ogg::{OggDemuxerConfig, OggDemuxerNode, OggMuxerConfig, OggMuxerNode};
use super::webm::{WebMMuxerConfig, WebMMuxerNode};
use crate::test_utils::{
    assert_state_initializing, assert_state_running, assert_state_stopped,
    create_test_binary_packet, create_test_context,
};
use bytes::Bytes;
use std::collections::HashMap;
use std::path::Path;
use streamkit_core::node::ProcessorNode;
use streamkit_core::types::Packet;
use tokio::sync::mpsc;

/// Helper to read test audio files
fn read_sample_file(filename: &str) -> Vec<u8> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-nodes should live under workspace_root/crates/nodes");
    let path = repo_root.join("samples/audio/system").join(filename);
    std::fs::read(&path).unwrap_or_else(|_| panic!("Failed to read test file: {}", path.display()))
}

/// Helper to create a mock Opus packet for testing
/// This creates a minimal valid Opus packet (silence)
fn create_mock_opus_packet() -> Packet {
    // Minimal Opus packet (20ms of silence, mono, 48kHz)
    // Opus packet format: TOC byte + compressed data
    // TOC: 0xFC = CELT-only mode, 20ms, mono
    let opus_data = vec![0xFC, 0xF8]; // TOC + minimal payload
    Packet::Binary { data: Bytes::from(opus_data), content_type: None, metadata: None }
}

#[cfg(feature = "symphonia")]
#[test]
fn test_symphonia_ogg_reader_opens_opus_file() {
    use symphonia::core::formats::FormatReader;

    let data = read_sample_file("speech_2m.opus");
    let source = symphonia::core::io::ReadOnlySource::new(std::io::Cursor::new(data));
    let mss = symphonia::core::io::MediaSourceStream::new(
        Box::new(source),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );

    let format_opts = symphonia::core::formats::FormatOptions::default();
    let mut reader =
        symphonia::default::formats::OggReader::try_new(mss, &format_opts).expect("open Ogg/Opus");

    let packet = reader.next_packet().expect("read first packet");
    assert!(!packet.data.is_empty(), "first packet should contain data");
}

#[tokio::test]
async fn test_ogg_muxer_basic() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    // Create OGG muxer node
    let config = OggMuxerConfig { stream_serial: 12345, ..Default::default() };
    let node = OggMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send some mock Opus packets
    for _ in 0..5 {
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    // Verify output
    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "OGG muxer should produce output packets");

    // Verify it's Binary packets with audio/ogg content type
    for packet in &output_packets {
        match packet {
            Packet::Binary { content_type, .. } => {
                assert_eq!(
                    content_type.as_deref(),
                    Some("audio/ogg"),
                    "OGG output should have audio/ogg content type"
                );
            },
            _ => panic!("Expected Binary packet from OGG muxer"),
        }
    }

    println!("✅ OGG muxer produced {} output packets", output_packets.len());
}

#[tokio::test]
async fn test_ogg_muxer_multiple_packets() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = OggMuxerConfig::default();
    let node = OggMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send multiple Opus packets
    for i in 0..20 {
        tracing::debug!("Sending Opus packet {}", i);
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Should produce output from multiple input packets");

    println!(
        "✅ OGG muxer handled 20 input packets, produced {} output packets",
        output_packets.len()
    );
}

#[tokio::test]
async fn test_ogg_demuxer_basic() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    // Create OGG demuxer node
    let node = OggDemuxerNode::new(OggDemuxerConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Read and send OGG test file
    let ogg_data = read_sample_file("sample.ogg");
    let packet = create_test_binary_packet(ogg_data);
    input_tx.send(packet).await.unwrap();

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    // Verify output
    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "OGG demuxer should extract packets");

    // Verify we got Binary packets (Opus-encoded)
    for packet in &output_packets {
        match packet {
            Packet::Binary { data, .. } => {
                assert!(!data.is_empty(), "Extracted Opus packets should have data");
            },
            _ => panic!("Expected Binary packet (Opus) from OGG demuxer"),
        }
    }

    println!("✅ OGG demuxer extracted {} Opus packets", output_packets.len());
}

#[tokio::test]
async fn test_ogg_demuxer_multiple_chunks() {
    // Test that demuxer can handle OGG data split across multiple packets
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let node = OggDemuxerNode::new(OggDemuxerConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Read OGG file and split into chunks
    let ogg_data = read_sample_file("sample.ogg");
    let chunk_size = ogg_data.len() / 4;

    for i in 0..4 {
        let start = i * chunk_size;
        let end = if i == 3 { ogg_data.len() } else { (i + 1) * chunk_size };
        let chunk = ogg_data[start..end].to_vec();
        let packet = create_test_binary_packet(chunk);
        input_tx.send(packet).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    // Verify we got output even with chunked input
    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Should extract packets even when input is chunked");

    println!("✅ OGG demuxer handled chunked input, extracted {} packets", output_packets.len());
}

#[tokio::test]
async fn test_ogg_roundtrip() {
    // Test muxing and then demuxing
    // Step 1: Mux some Opus packets to OGG
    let (mux_input_tx, mux_input_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_input_rx);

    let (mux_context, mux_mock_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);

    let mux_config = OggMuxerConfig { stream_serial: 99999, ..Default::default() };
    let mux_node = OggMuxerNode::new(mux_config);

    let mux_handle = tokio::spawn(async move { Box::new(mux_node).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    // Send Opus packets to muxer
    for _ in 0..10 {
        mux_input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(mux_input_tx);
    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    let muxed_packets = mux_mock_sender.get_packets_for_pin("out").await;
    assert!(!muxed_packets.is_empty(), "Muxer should produce output");

    println!("✅ Muxed {} OGG packets", muxed_packets.len());

    // Step 2: Demux the OGG data
    let (demux_input_tx, demux_input_rx) = mpsc::channel(10);
    let mut demux_inputs = HashMap::new();
    demux_inputs.insert("in".to_string(), demux_input_rx);

    let (demux_context, demux_mock_sender, mut demux_state_rx) =
        create_test_context(demux_inputs, 10);

    let demux_node = OggDemuxerNode::new(OggDemuxerConfig::default());

    let demux_handle = tokio::spawn(async move { Box::new(demux_node).run(demux_context).await });

    assert_state_initializing(&mut demux_state_rx).await;
    assert_state_running(&mut demux_state_rx).await;

    // Send muxed OGG packets to demuxer
    for packet in muxed_packets {
        demux_input_tx.send(packet).await.unwrap();
    }

    drop(demux_input_tx);
    assert_state_stopped(&mut demux_state_rx).await;
    demux_handle.await.unwrap().unwrap();

    let demuxed_packets = demux_mock_sender.get_packets_for_pin("out").await;
    assert!(!demuxed_packets.is_empty(), "Demuxer should extract packets from muxed data");

    println!("✅ Demuxed {} Opus packets from muxed OGG", demuxed_packets.len());
}

#[tokio::test]
async fn test_webm_muxer_basic() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    // Create WebM muxer node
    let config = WebMMuxerConfig::default();
    let node = WebMMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send some mock Opus packets
    for _ in 0..5 {
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    // Verify output
    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM muxer should produce output packets");

    // WebM/Opus should include OpusHead in CodecPrivate for broad browser compatibility (Firefox).
    let mut combined = Vec::new();
    for packet in &output_packets {
        if let Packet::Binary { data, .. } = packet {
            combined.extend_from_slice(data);
        }
    }
    assert!(
        combined.windows(b"OpusHead".len()).any(|w| w == b"OpusHead"),
        "WebM output should include OpusHead codec private"
    );

    // Verify it's Binary packets with audio/webm content type
    for packet in &output_packets {
        match packet {
            Packet::Binary { content_type, .. } => {
                assert!(
                    content_type.as_deref().is_some_and(|ct| ct.starts_with("audio/webm")),
                    "WebM output should have audio/webm content type, got: {content_type:?}"
                );
            },
            _ => panic!("Expected Binary packet from WebM muxer"),
        }
    }

    println!("✅ WebM muxer produced {} output packets", output_packets.len());
}

#[tokio::test]
async fn test_webm_muxer_multiple_packets() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = WebMMuxerConfig::default();
    let node = WebMMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send multiple Opus packets
    for i in 0..15 {
        tracing::debug!("Sending Opus packet {} to WebM muxer", i);
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM should produce output from multiple input packets");

    // Verify the output data is not empty
    let total_bytes: usize = output_packets
        .iter()
        .map(|p| match p {
            Packet::Binary { data, .. } => data.len(),
            _ => 0,
        })
        .sum();

    assert!(total_bytes > 0, "WebM output should contain data");

    println!(
        "✅ WebM muxer handled 15 input packets, produced {} output packets ({} bytes total)",
        output_packets.len(),
        total_bytes
    );
}

#[tokio::test]
async fn test_webm_sliding_window() {
    // Test that WebM muxer handles long streams with sliding window
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    // Create config with smaller chunk size for testing
    let config = WebMMuxerConfig {
        chunk_size: 1024, // Small chunk size to force frequent flushes
        ..Default::default()
    };
    let node = WebMMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send many packets to test sliding window behavior
    for i in 0..50 {
        if i % 10 == 0 {
            tracing::debug!("Sent {} packets to WebM muxer", i);
        }
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Should handle many packets with sliding window");

    println!(
        "✅ WebM muxer handled 50 packets with sliding window, produced {} output packets",
        output_packets.len()
    );
}
