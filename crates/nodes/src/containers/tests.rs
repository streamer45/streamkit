// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for container nodes (OGG, WAV, WebM)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]

use super::ogg::{OggDemuxerConfig, OggDemuxerNode, OggMuxerConfig, OggMuxerNode};
use super::webm::{WebMMuxerConfig, WebMMuxerNode, WebMStreamingMode};
use crate::test_utils::{
    assert_state_initializing, assert_state_running, assert_state_stopped,
    create_test_binary_packet, create_test_context,
};
use bytes::Bytes;
use std::collections::HashMap;
use std::path::Path;
use streamkit_core::node::ProcessorNode;
use streamkit_core::types::{AudioCodec, EncodedAudioFormat, Packet, PacketType};
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

    let (mut context, mock_sender, mut state_rx) = create_test_context(inputs, 10);
    context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
    );

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

    let (mut context, mock_sender, mut state_rx) = create_test_context(inputs, 10);
    context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
    );

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

    let (mut context, mock_sender, mut state_rx) = create_test_context(inputs, 10);
    context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
    );

    // Create config (chunk_size was removed — the default streaming mode
    // flushes incrementally on every frame write).
    let config = WebMMuxerConfig::default();
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

/// Smoke test: video-only VP9 frames muxed into WebM produce non-empty, parseable output.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_vp9_video_only() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{PacketMetadata, PixelFormat};

    // ---- Step 1: Encode some raw I420 frames to VP9 ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping VP9 video-only mux test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let frame_count = 5u64;
    for i in 0..frame_count {
        let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(i * 33_333),
            duration_us: Some(33_333),
            sequence: Some(i),
            keyframe: Some(true),
        });
        enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Mux the encoded VP9 packets into WebM ----

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    // Only video, no audio
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(streamkit_core::types::EncodedVideoFormat {
            codec: streamkit_core::types::VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );
    let mux_config =
        WebMMuxerConfig { video_width: 64, video_height: 64, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    for packet in encoded_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    // ---- Step 3: Validate output ----

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM muxer produced no output");

    // Collect all output bytes
    let mut webm_bytes = Vec::new();
    for packet in &output_packets {
        if let Packet::Binary { data, .. } = packet {
            webm_bytes.extend_from_slice(data);
        }
    }

    assert!(!webm_bytes.is_empty(), "WebM output is empty");
    // WebM/EBML files start with 0x1A45DFA3 (EBML header element ID)
    assert!(webm_bytes.len() >= 4, "WebM output too small: {} bytes", webm_bytes.len());
    assert_eq!(
        &webm_bytes[..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "WebM output does not start with EBML header"
    );

    // Verify content type
    if let Packet::Binary { content_type, .. } = &output_packets[0] {
        let ct = content_type.as_ref().expect("content_type should be set");
        assert_eq!(ct.as_ref(), "video/webm; codecs=\"vp9\"");
    }

    println!(
        "✅ WebM video-only mux test passed: {} output packets, {} total bytes",
        output_packets.len(),
        webm_bytes.len()
    );
}

/// Test that muxer returns an error if no inputs are connected.
#[tokio::test]
async fn test_webm_mux_no_inputs_fails() {
    let mux_inputs = HashMap::new();
    let (mux_context, _mux_sender, _mux_state_rx) = create_test_context(mux_inputs, 10);
    let muxer = WebMMuxerNode::new(WebMMuxerConfig::default());
    let result = Box::new(muxer).run(mux_context).await;
    assert!(result.is_err(), "Expected error when no inputs are connected");
}

/// Smoke test: combined audio (Opus) + video (VP9) muxed into a single WebM container.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_audio_and_video() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec};

    // ---- Step 1: Encode a few raw I420 frames to VP9 ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping combined audio+video mux test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let frame_count = 5u64;
    for i in 0..frame_count {
        let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(i * 33_333),
            duration_us: Some(33_333),
            sequence: Some(i),
            keyframe: Some(true),
        });
        enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_video_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_video_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Mux audio + video into WebM ----

    let (mux_audio_tx, mux_audio_rx) = mpsc::channel(10);
    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_audio_rx);
    mux_inputs.insert("in_1".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
    );
    mux_context.input_types.insert(
        "in_1".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    let mux_config =
        WebMMuxerConfig { video_width: 64, video_height: 64, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    // Send audio packets
    for _ in 0..10 {
        mux_audio_tx.send(create_mock_opus_packet()).await.unwrap();
    }
    // Send video packets
    for packet in encoded_video_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_audio_tx);
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    // ---- Step 3: Validate output ----

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM muxer produced no output for audio+video");

    // Collect all output bytes
    let mut webm_bytes = Vec::new();
    for packet in &output_packets {
        if let Packet::Binary { data, .. } = packet {
            webm_bytes.extend_from_slice(data);
        }
    }

    // Verify EBML header
    assert!(webm_bytes.len() >= 4, "WebM output too small: {} bytes", webm_bytes.len());
    assert_eq!(
        &webm_bytes[..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "WebM output does not start with EBML header"
    );

    // Verify content type includes both codecs
    if let Packet::Binary { content_type, .. } = &output_packets[0] {
        let ct = content_type.as_ref().expect("content_type should be set");
        assert_eq!(
            ct.as_ref(),
            "video/webm; codecs=\"vp9,opus\"",
            "Combined mux should include both codecs"
        );
    }

    // Verify OpusHead is present (audio codec private)
    assert!(
        webm_bytes.windows(b"OpusHead".len()).any(|w| w == b"OpusHead"),
        "Combined WebM output should include OpusHead codec private"
    );

    println!(
        "WebM combined audio+video mux test passed: {} output packets, {} total bytes",
        output_packets.len(),
        webm_bytes.len()
    );
}

/// Test that the WebM muxer auto-detects video dimensions from the first VP9 keyframe
/// when `video_width` and `video_height` are both 0.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_vp9_auto_detect_dimensions() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec};

    // ---- Step 1: Encode raw frames to VP9 ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping VP9 auto-detect test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let frame_count = 3u64;
    for i in 0..frame_count {
        let mut frame = create_test_video_frame(128, 96, PixelFormat::I420, 32);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(i * 33_333),
            duration_us: Some(33_333),
            sequence: Some(i),
            keyframe: Some(true),
        });
        enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Mux with auto-detect dimensions (video_width=0, video_height=0) ----

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    // Single pin for video-only.  With width/height = 0 the muxer uses a single pin.
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    // Explicitly set 0x0 to trigger auto-detection
    let mux_config =
        WebMMuxerConfig { video_width: 0, video_height: 0, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    for packet in encoded_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    // ---- Step 3: Validate output ----

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(
        !output_packets.is_empty(),
        "WebM muxer produced no output with auto-detected dimensions"
    );

    // Verify EBML header is valid
    let mut webm_bytes = Vec::new();
    for packet in &output_packets {
        if let Packet::Binary { data, .. } = packet {
            webm_bytes.extend_from_slice(data);
        }
    }
    assert!(webm_bytes.len() >= 4, "WebM output too small: {} bytes", webm_bytes.len());
    assert_eq!(
        &webm_bytes[..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "WebM output does not start with EBML header"
    );

    println!(
        "WebM VP9 auto-detect dimensions test passed: {} output packets, {} total bytes",
        output_packets.len(),
        webm_bytes.len()
    );
}

/// Test that WebM muxer works in File mode (seekable temp-file backed).
/// File mode produces a single output packet after finalization with full
/// duration and seeking info.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_file_mode() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec};

    // ---- Step 1: Encode raw I420 frames to VP9 ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping VP9 File mode mux test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let frame_count = 5u64;
    for i in 0..frame_count {
        let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(i * 33_333),
            duration_us: Some(33_333),
            sequence: Some(i),
            keyframe: Some(true),
        });
        enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Mux in File mode ----

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    let mux_config = WebMMuxerConfig {
        video_width: 64,
        video_height: 64,
        streaming_mode: WebMStreamingMode::File,
        ..WebMMuxerConfig::default()
    };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    for packet in encoded_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    // ---- Step 3: Validate File mode output ----

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    // File mode emits a single packet after finalization
    assert!(!output_packets.is_empty(), "WebM File mode muxer produced no output");

    let mut webm_bytes = Vec::new();
    for packet in &output_packets {
        if let Packet::Binary { data, .. } = packet {
            webm_bytes.extend_from_slice(data);
        }
    }

    assert!(webm_bytes.len() >= 4, "WebM File mode output too small");
    assert_eq!(
        &webm_bytes[..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "WebM File mode output does not start with EBML header"
    );

    println!(
        "WebM File mode mux test passed: {} output packets, {} total bytes",
        output_packets.len(),
        webm_bytes.len()
    );
}

/// Test muxer behaviour when the first video packet is not a keyframe
/// (e.g. truncated or non-keyframe VP9 data).
#[tokio::test]
async fn test_webm_mux_non_keyframe_first_video() {
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, VideoCodec};

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, _mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    // video_width/height = 0 means auto-detect from first keyframe.
    // Sending non-keyframe data first should not panic.
    let mux_config =
        WebMMuxerConfig { video_width: 0, video_height: 0, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    // Send a small non-keyframe packet (random bytes, not a valid VP9 keyframe).
    // The muxer should handle this gracefully (skip or error, not panic).
    let non_kf = Packet::Binary {
        data: Bytes::from_static(&[0x00, 0x01, 0x02, 0x03]),
        content_type: None,
        metadata: Some(PacketMetadata {
            timestamp_us: Some(0),
            duration_us: Some(33_333),
            sequence: Some(0),
            keyframe: Some(false),
        }),
    };
    let _ = mux_video_tx.send(non_kf).await;

    // Close the channel — the muxer should finish without panicking.
    drop(mux_video_tx);

    let result = mux_handle.await.unwrap();
    // The muxer may return Ok or Err depending on whether it waits
    // for a keyframe forever vs. giving up, but it should not panic.
    let _ = result;
    println!("WebM non-keyframe first video test passed (no panic)");
}

/// Test that sending truncated/corrupt VP9 data does not panic the muxer.
#[tokio::test]
async fn test_webm_mux_truncated_vp9_header() {
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, VideoCodec};

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, _mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    // Auto-detect mode — send corrupt VP9 data
    let mux_config =
        WebMMuxerConfig { video_width: 0, video_height: 0, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    // Send a packet flagged as keyframe but with truncated/corrupt VP9 data
    // (too short for `parse_vp9_keyframe_dimensions` to extract dimensions).
    let truncated = Packet::Binary {
        data: Bytes::from_static(&[0x82, 0x49, 0x83]), // partial sync code
        content_type: None,
        metadata: Some(PacketMetadata {
            timestamp_us: Some(0),
            duration_us: Some(33_333),
            sequence: Some(0),
            keyframe: Some(true),
        }),
    };
    let _ = mux_video_tx.send(truncated).await;
    drop(mux_video_tx);

    let result = mux_handle.await.unwrap();
    // Should not panic; may return an error about dimension detection.
    let _ = result;
    println!("WebM truncated VP9 header test passed (no panic)");
}

/// Regression test: in dynamic pipelines `NodeContext::input_types` is empty,
/// so the WebM muxer must fall back to first-packet inspection to classify
/// inputs as audio vs video.  Previously all inputs defaulted to audio,
/// causing "multiple audio inputs detected" when a video encoder was connected.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_dynamic_pipeline_classifies_inputs_from_packets() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{PacketMetadata, PixelFormat};

    // ---- Step 1: Encode a real VP9 keyframe ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);

    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping dynamic classification test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
    frame.metadata = Some(PacketMetadata {
        timestamp_us: Some(0),
        duration_us: Some(33_333),
        sequence: Some(0),
        keyframe: Some(true),
    });
    enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_video_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_video_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Mux with empty input_types (dynamic pipeline simulation) ----

    let (mux_audio_tx, mux_audio_rx) = mpsc::channel(10);
    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_audio_rx);
    mux_inputs.insert("in_1".to_string(), mux_video_rx);

    // create_test_context sets input_types to HashMap::new() — exactly the
    // dynamic pipeline case we're testing.  Do NOT populate input_types here.
    let (mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    assert!(
        mux_context.input_types.is_empty(),
        "Test precondition: input_types must be empty to simulate dynamic pipeline"
    );

    let mux_config =
        WebMMuxerConfig { video_width: 64, video_height: 64, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;

    // Send audio packets (content_type: None → classified as audio via packet inspection)
    for _ in 0..5 {
        mux_audio_tx.send(create_mock_opus_packet()).await.unwrap();
    }
    // Send video packets (content_type: Some("video/vp9") → classified as video)
    for packet in encoded_video_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_audio_tx);
    drop(mux_video_tx);

    // The key assertion: the node should reach Running state (not fail with
    // "multiple audio inputs detected") and then stop cleanly.
    assert_state_running(&mut mux_state_rx).await;
    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    // Verify we got output packets (the muxer actually produced WebM data)
    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(
        !output_packets.is_empty(),
        "WebM muxer should produce output when classifying inputs via packet inspection"
    );

    println!("WebM dynamic pipeline input classification test passed");
}

/// Regression test: video-only muxing in a dynamic pipeline (empty input_types)
/// should classify the single input as video from content_type, not default to audio.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_dynamic_pipeline_video_only() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::types::{PacketMetadata, PixelFormat};

    // ---- Step 1: Encode a real VP9 keyframe ----

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);

    let encoder_config = Vp9EncoderConfig {
        keyframe_interval: 1,
        bitrate_kbps: 800,
        threads: 1,
        ..Default::default()
    };
    let encoder = match Vp9EncoderNode::new(encoder_config) {
        Ok(enc) => enc,
        Err(e) => {
            eprintln!("Skipping dynamic video-only test: encoder not available ({e})");
            return;
        },
    };
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
    frame.metadata = Some(PacketMetadata {
        timestamp_us: Some(0),
        duration_us: Some(33_333),
        sequence: Some(0),
        keyframe: Some(true),
    });
    enc_input_tx.send(Packet::Video(frame)).await.unwrap();
    drop(enc_input_tx);

    assert_state_stopped(&mut enc_state_rx).await;
    enc_handle.await.unwrap().unwrap();

    let encoded_video_packets = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_video_packets.is_empty(), "VP9 encoder produced no packets");

    // ---- Step 2: Video-only mux with empty input_types ----

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    assert!(mux_context.input_types.is_empty());

    // video_width/height = 0 triggers auto-detect from the first VP9 keyframe
    let mux_config =
        WebMMuxerConfig { video_width: 0, video_height: 0, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;

    // Send video packets — should be classified as video via content_type inspection
    for packet in encoded_video_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_video_tx);

    assert_state_running(&mut mux_state_rx).await;
    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(
        !output_packets.is_empty(),
        "WebM muxer should produce output for video-only dynamic pipeline"
    );

    println!("WebM dynamic pipeline video-only classification test passed");
}
