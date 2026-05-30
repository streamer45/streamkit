// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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

fn read_sample_file(filename: &str) -> Vec<u8> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-nodes should live under workspace_root/crates/nodes");
    let path = repo_root.join("samples/audio/system").join(filename);
    std::fs::read(&path).unwrap_or_else(|_| panic!("Failed to read test file: {}", path.display()))
}

fn create_mock_opus_packet() -> Packet {
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

    let config = OggMuxerConfig { stream_serial: 12345, ..Default::default() };
    let node = OggMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    for _ in 0..5 {
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "OGG muxer should produce output packets");

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

    let node = OggDemuxerNode::new(OggDemuxerConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    let ogg_data = read_sample_file("sample.ogg");
    let packet = create_test_binary_packet(ogg_data);
    input_tx.send(packet).await.unwrap();

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "OGG demuxer should extract packets");

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
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let node = OggDemuxerNode::new(OggDemuxerConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

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

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Should extract packets even when input is chunked");

    println!("✅ OGG demuxer handled chunked input, extracted {} packets", output_packets.len());
}

#[tokio::test]
async fn test_ogg_roundtrip() {
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

    let config = WebMMuxerConfig::default();
    let node = WebMMuxerNode::new(config);

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    for _ in 0..5 {
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

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

    for i in 0..15 {
        tracing::debug!("Sending Opus packet {} to WebM muxer", i);
        input_tx.send(create_mock_opus_packet()).await.unwrap();
    }

    drop(input_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM should produce output from multiple input packets");

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

    for _ in 0..10 {
        mux_audio_tx.send(create_mock_opus_packet()).await.unwrap();
    }
    for packet in encoded_video_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_audio_tx);
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM muxer produced no output for audio+video");

    // Collect all output bytes
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

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(
        !output_packets.is_empty(),
        "WebM muxer produced no output with auto-detected dimensions"
    );

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
/// File mode streams the finalized temp file back downstream in bounded chunks;
/// the concatenated Binary packets form the full container with duration and
/// seeking info, and no single packet exceeds the chunk size.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_file_mode() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use std::num::NonZeroUsize;
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec};

    // A tiny chunk size so even this small muxed file splits into several
    // bounded packets, exercising the real chunked read-back path.
    let chunk_size = 64usize;

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
        finalize_chunk_size: NonZeroUsize::new(chunk_size),
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

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    let binary_packets: Vec<&Bytes> = output_packets
        .iter()
        .filter_map(|p| match p {
            Packet::Binary { data, .. } => Some(data),
            _ => None,
        })
        .collect();

    assert!(
        binary_packets.len() > 1,
        "File-mode output exceeding one chunk must be emitted as multiple packets, got {}",
        binary_packets.len()
    );

    let mut webm_bytes = Vec::new();
    for data in &binary_packets {
        assert!(
            data.len() <= chunk_size,
            "no finalized File-mode packet may exceed the configured chunk size"
        );
        webm_bytes.extend_from_slice(data);
    }

    assert!(
        webm_bytes.len() > chunk_size,
        "test should produce more than one chunk worth of output"
    );
    assert_eq!(
        binary_packets.len(),
        webm_bytes.len().div_ceil(chunk_size),
        "File-mode must emit exactly ceil(total / configured chunk size) packets"
    );
    assert_eq!(
        &webm_bytes[..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "WebM File mode output does not start with EBML header"
    );
}

/// Test that MP4 File mode honours a configured `finalize_chunk_size`: the
/// output is streamed back as multiple `Packet::Binary` chunks, none exceeding
/// the configured size, the chunk count matches the configured size exactly,
/// and their concatenation is the full MP4 (starting with the `ftyp` box).
#[cfg(all(feature = "mp4", feature = "openh264"))]
#[tokio::test]
async fn test_mp4_mux_file_mode_streams_multiple_chunks() {
    use super::mp4::{Mp4MuxerConfig, Mp4MuxerNode, Mp4StreamingMode};
    use crate::test_utils::create_textured_video_frame;
    use crate::video::openh264::{OpenH264EncoderConfig, OpenH264EncoderNode};
    use std::num::NonZeroUsize;
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, VideoCodec};

    // A smaller-than-default chunk size to prove the override is wired through
    // without needing to mux an unreasonably large file.
    let chunk_size = 64 * 1024usize;
    let width = 640u32;
    let height = 480u32;
    let frame_count = 12u64;

    let (enc_input_tx, enc_input_rx) = mpsc::channel(64);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 64);
    // High bitrate + every-frame keyframes so the textured frames below mux to
    // well over one chunk, exercising the chunked read-back.
    let encoder_config =
        OpenH264EncoderConfig { bitrate_kbps: 8_000, max_frame_rate: 30.0, gop_size: 1 };
    let encoder = OpenH264EncoderNode::new(encoder_config).unwrap();
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    for i in 0..frame_count {
        let shift = u8::try_from(i.wrapping_mul(11) % 256).unwrap();
        let mut frame = create_textured_video_frame(width, height, shift);
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
    assert!(!encoded_packets.is_empty(), "OpenH264 encoder produced no packets");

    let (mux_video_tx, mux_video_rx) = mpsc::channel(encoded_packets.len().max(1));
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 64);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::H264,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    );

    let mux_config = Mp4MuxerConfig {
        mode: Mp4StreamingMode::File,
        video_width: u16::try_from(width).unwrap(),
        video_height: u16::try_from(height).unwrap(),
        video_codec: Some(VideoCodec::H264),
        finalize_chunk_size: NonZeroUsize::new(chunk_size),
        ..Mp4MuxerConfig::default()
    };
    let muxer = Mp4MuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;
    assert_state_running(&mut mux_state_rx).await;

    for packet in encoded_packets {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_video_tx);

    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    let binary_packets: Vec<&Bytes> = output_packets
        .iter()
        .filter_map(|p| match p {
            Packet::Binary { data, .. } => Some(data),
            _ => None,
        })
        .collect();

    assert!(
        binary_packets.len() > 1,
        "File-mode output exceeding one chunk must be emitted as multiple packets, got {}",
        binary_packets.len()
    );

    let mut mp4_bytes = Vec::new();
    for data in &binary_packets {
        assert!(
            data.len() <= chunk_size,
            "no finalized File-mode packet may exceed the configured chunk size"
        );
        mp4_bytes.extend_from_slice(data);
    }

    assert!(
        mp4_bytes.len() > chunk_size,
        "test should produce more than one chunk worth of output"
    );
    assert_eq!(
        binary_packets.len(),
        mp4_bytes.len().div_ceil(chunk_size),
        "File-mode must emit exactly ceil(total / configured chunk size) packets"
    );
    assert_eq!(&mp4_bytes[4..8], b"ftyp", "MP4 File mode output must start with an ftyp box");
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
    use streamkit_core::pins::PinManagementMessage;
    use streamkit_core::types::{
        AudioCodec, EncodedAudioFormat, EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec,
    };

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

    let (mux_audio_tx, mux_audio_rx) = mpsc::channel(10);
    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_audio_rx);
    mux_inputs.insert("in_1".to_string(), mux_video_rx);

    // Use create_test_context_with_pin_mgmt so we can send InputTypeResolved
    // messages, simulating what the engine does in connect_nodes().
    let (mux_context, mux_sender, mut mux_state_rx, pin_mgmt_tx) =
        crate::test_utils::create_test_context_with_pin_mgmt(mux_inputs, 10);
    assert!(
        mux_context.input_types.is_empty(),
        "Test precondition: input_types must be empty to simulate dynamic pipeline"
    );

    let mux_config =
        WebMMuxerConfig { video_width: 64, video_height: 64, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;

    // Send InputTypeResolved for both pins (simulates engine behaviour).
    pin_mgmt_tx
        .send(PinManagementMessage::InputTypeResolved {
            pin_name: "in".to_string(),
            packet_type: streamkit_core::types::PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
        })
        .await
        .unwrap();
    pin_mgmt_tx
        .send(PinManagementMessage::InputTypeResolved {
            pin_name: "in_1".to_string(),
            packet_type: streamkit_core::types::PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Vp9,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        })
        .await
        .unwrap();
    drop(pin_mgmt_tx);

    for _ in 0..5 {
        mux_audio_tx.send(create_mock_opus_packet()).await.unwrap();
    }
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

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(
        !output_packets.is_empty(),
        "WebM muxer should produce output when classifying inputs via InputTypeResolved"
    );

    println!("WebM dynamic pipeline InputTypeResolved classification test passed");
}

/// Regression test: video-only muxing in a dynamic pipeline (empty input_types)
/// should classify the single input as video from content_type, not default to audio.
#[cfg(feature = "vp9")]
#[tokio::test]
async fn test_webm_mux_dynamic_pipeline_video_only() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::vp9::{Vp9EncoderConfig, Vp9EncoderNode};
    use streamkit_core::pins::PinManagementMessage;
    use streamkit_core::types::{EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec};

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

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mux_context, mux_sender, mut mux_state_rx, pin_mgmt_tx) =
        crate::test_utils::create_test_context_with_pin_mgmt(mux_inputs, 10);
    assert!(mux_context.input_types.is_empty());

    // video_width/height = 0 triggers auto-detect from the first VP9 keyframe
    let mux_config =
        WebMMuxerConfig { video_width: 0, video_height: 0, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;

    // Send InputTypeResolved so the muxer classifies this pin as video.
    pin_mgmt_tx
        .send(PinManagementMessage::InputTypeResolved {
            pin_name: "in".to_string(),
            packet_type: streamkit_core::types::PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Vp9,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        })
        .await
        .unwrap();
    drop(pin_mgmt_tx);

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

/// Encode a few AV1 frames, mux them into WebM, and verify the output starts
/// with EBML magic bytes and uses the `V_AV1` codec (content_type includes "av1").
#[cfg(feature = "av1")]
#[tokio::test]
async fn test_webm_mux_av1_video_only() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::av1::{Av1EncoderConfig, Av1EncoderNode};
    use streamkit_core::types::{PacketMetadata, PixelFormat};

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Av1EncoderConfig {
        speed: 10,
        quantizer: 80,
        threads: 1,
        low_latency: true,
        bitrate_kbps: 0,
        ..Default::default()
    };
    let encoder = Av1EncoderNode::new(encoder_config).unwrap();
    let enc_handle: tokio::task::JoinHandle<Result<(), streamkit_core::StreamKitError>> =
        tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    let frame_count = 5u64;
    for i in 0..frame_count {
        let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
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
    assert!(!encoded_packets.is_empty(), "AV1 encoder produced no packets");

    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_video_rx);

    let (mut mux_context, mux_sender, mut mux_state_rx) = create_test_context(mux_inputs, 10);
    mux_context.input_types.insert(
        "in".to_string(),
        PacketType::EncodedVideo(streamkit_core::types::EncodedVideoFormat {
            codec: streamkit_core::types::VideoCodec::Av1,
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

    // Verify the matroska codec ID is V_AV1 by scanning the output bytes.
    let v_av1_bytes = b"V_AV1";
    let found_v_av1 = webm_bytes.windows(v_av1_bytes.len()).any(|w| w == v_av1_bytes);
    assert!(found_v_av1, "WebM output does not contain V_AV1 codec ID");

    // Verify content type includes "av1"
    if let Packet::Binary { content_type, .. } = &output_packets[0] {
        let ct = content_type.as_ref().expect("content_type should be set");
        assert_eq!(ct.as_ref(), "video/webm; codecs=\"av1\"");
    }

    println!(
        "WebM AV1 video-only mux test passed: {} output packets, {} total bytes",
        output_packets.len(),
        webm_bytes.len()
    );
}

/// Verify that AV1 codec detection works via `InputTypeResolved` in a dynamic
/// pipeline (empty `input_types`).  The muxer should set `video_is_av1 = true`
/// from the resolved type info, producing content_type `"video/webm; codecs=\"av1,opus\""`.
#[cfg(feature = "av1")]
#[tokio::test]
async fn test_webm_mux_av1_via_input_type_resolved() {
    use crate::test_utils::create_test_video_frame;
    use crate::video::av1::{Av1EncoderConfig, Av1EncoderNode};
    use streamkit_core::pins::PinManagementMessage;
    use streamkit_core::types::{
        AudioCodec, EncodedAudioFormat, EncodedVideoFormat, PacketMetadata, PixelFormat, VideoCodec,
    };

    let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
    let mut enc_inputs = HashMap::new();
    enc_inputs.insert("in".to_string(), enc_input_rx);

    let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
    let encoder_config = Av1EncoderConfig {
        speed: 10,
        quantizer: 80,
        threads: 1,
        low_latency: true,
        bitrate_kbps: 0,
        ..Default::default()
    };
    let encoder = Av1EncoderNode::new(encoder_config).unwrap();
    let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

    assert_state_initializing(&mut enc_state_rx).await;
    assert_state_running(&mut enc_state_rx).await;

    for i in 0..3u64 {
        let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
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

    let encoded_video = enc_sender.get_packets_for_pin("out").await;
    assert!(!encoded_video.is_empty(), "AV1 encoder produced no packets");

    let (mux_audio_tx, mux_audio_rx) = mpsc::channel(10);
    let (mux_video_tx, mux_video_rx) = mpsc::channel(10);
    let mut mux_inputs = HashMap::new();
    mux_inputs.insert("in".to_string(), mux_audio_rx);
    mux_inputs.insert("in_1".to_string(), mux_video_rx);

    let (mux_context, mux_sender, mut mux_state_rx, pin_mgmt_tx) =
        crate::test_utils::create_test_context_with_pin_mgmt(mux_inputs, 10);
    assert!(mux_context.input_types.is_empty());

    let mux_config =
        WebMMuxerConfig { video_width: 64, video_height: 64, ..WebMMuxerConfig::default() };
    let muxer = WebMMuxerNode::new(mux_config);
    let mux_handle = tokio::spawn(async move { Box::new(muxer).run(mux_context).await });

    assert_state_initializing(&mut mux_state_rx).await;

    // Deliver AV1 + Opus type info via InputTypeResolved.
    pin_mgmt_tx
        .send(PinManagementMessage::InputTypeResolved {
            pin_name: "in".to_string(),
            packet_type: PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
        })
        .await
        .unwrap();
    pin_mgmt_tx
        .send(PinManagementMessage::InputTypeResolved {
            pin_name: "in_1".to_string(),
            packet_type: PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        })
        .await
        .unwrap();
    drop(pin_mgmt_tx);

    for _ in 0..3 {
        mux_audio_tx.send(create_mock_opus_packet()).await.unwrap();
    }
    for packet in encoded_video {
        mux_video_tx.send(packet).await.unwrap();
    }
    drop(mux_audio_tx);
    drop(mux_video_tx);

    assert_state_running(&mut mux_state_rx).await;
    assert_state_stopped(&mut mux_state_rx).await;
    mux_handle.await.unwrap().unwrap();

    let output_packets = mux_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "WebM muxer produced no output");

    if let Packet::Binary { content_type, .. } = &output_packets[0] {
        let ct = content_type.as_ref().expect("content_type should be set");
        assert_eq!(
            ct.as_ref(),
            "video/webm; codecs=\"av1,opus\"",
            "AV1 codec detection via InputTypeResolved should produce av1,opus content type"
        );
    } else {
        panic!("Expected Binary packet from WebM muxer");
    }

    println!("WebM AV1 via InputTypeResolved test passed");
}
