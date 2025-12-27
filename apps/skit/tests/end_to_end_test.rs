// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

use axum::http::StatusCode;
use reqwest::multipart;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::Path;
use streamkit_server::Config;
use tokio::fs;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};

async fn start_test_server() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    // Find an available port by binding to port 0
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    // Start server in background using the existing listener
    let server_handle = tokio::spawn(async move {
        let (app, _state) = streamkit_server::server::create_app(Config::default());
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    Some((addr, server_handle))
}

#[tokio::test]
async fn test_double_volume_end_to_end() {
    // Initialize tracing for debugging
    let _ = tracing_subscriber::fmt::try_init();

    // Start test server
    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping end-to-end tests: local TCP bind not permitted");
        return;
    };

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-server should live under workspace_root/apps/skit");

    // Load pipeline configuration
    let pipeline_yaml =
        fs::read_to_string(repo_root.join("samples/pipelines/oneshot/double_volume.yml"))
            .await
            .expect("Failed to read pipeline YAML");

    // Load input audio file
    let audio_data = fs::read(repo_root.join("samples/audio/system/sample.ogg"))
        .await
        .expect("Failed to read input audio file");

    // Create multipart form
    let form = multipart::Form::new()
        .text("config", pipeline_yaml)
        .part("media", multipart::Part::bytes(audio_data).file_name("sample.ogg"));

    // Send request to server
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/process");

    let response =
        timeout(Duration::from_secs(30), async { client.post(&url).multipart(form).send().await })
            .await
            .expect("Request timed out")
            .expect("Failed to send request");

    // Verify response status
    assert_eq!(response.status(), StatusCode::OK, "Expected 200 OK, got: {}", response.status());

    // Verify content type
    let content_type = response
        .headers()
        .get("content-type")
        .expect("Missing content-type header")
        .to_str()
        .expect("Invalid content-type header");
    assert_eq!(content_type, "audio/ogg", "Unexpected content type");

    // Get response body
    let response_body = response.bytes().await.expect("Failed to read response body");

    // Validate the Ogg/Opus file by actually parsing and decoding it
    let cursor = Cursor::new(response_body.as_ref());
    let mut packet_reader = ogg::PacketReader::new(cursor);

    let mut opus_decoder = None;
    let mut total_samples_decoded = 0;
    let mut packet_count = 0;

    // Read all packets from the Ogg stream
    while let Some(packet) = packet_reader.read_packet().expect("Failed to read Ogg packet") {
        packet_count += 1;

        // Skip the OpusHead header packet
        if packet.data.len() >= 8 && &packet.data[0..8] == b"OpusHead" {
            // Verify OpusHead structure
            assert_eq!(packet.data[8], 1, "Invalid Opus version");
            continue;
        }

        // Skip the OpusTags packet
        if packet.data.len() >= 8 && &packet.data[0..8] == b"OpusTags" {
            continue;
        }

        // Initialize decoder on first audio packet
        if opus_decoder.is_none() {
            opus_decoder = Some(
                opus::Decoder::new(48000, opus::Channels::Stereo)
                    .expect("Failed to create Opus decoder"),
            );
        }

        // Decode the Opus packet
        if let Some(ref mut decoder) = opus_decoder {
            let mut output = vec![0i16; 48000]; // Max frame size
            let samples = decoder
                .decode(&packet.data, &mut output, false)
                .expect("Failed to decode Opus packet");
            total_samples_decoded += samples;
        }
    }

    // Verify we decoded a reasonable amount of audio
    assert!(packet_count > 0, "No packets found in Ogg stream");
    assert!(total_samples_decoded > 0, "No audio samples decoded from Opus packets");

    // The sample file is about 5 seconds, so we should have roughly 240k samples at 48kHz
    // Allow a wide range since exact duration may vary
    assert!(
        total_samples_decoded > 100_000 && total_samples_decoded < 500_000,
        "Unexpected number of samples decoded: {} (expected roughly 240k for a ~5s file)",
        total_samples_decoded
    );

    println!(
        "✅ End-to-end double volume pipeline test passed! Validated {} Ogg packets, decoded {} audio samples",
        packet_count,
        total_samples_decoded
    );
}

#[tokio::test]
async fn test_missing_config_field() {
    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping end-to-end tests: local TCP bind not permitted");
        return;
    };

    // Send request with only media, no config
    let audio_data = b"fake audio data";
    let form = multipart::Form::new()
        .part("media", multipart::Part::bytes(audio_data.to_vec()).file_name("test.ogg"));

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/process");

    let response = client.post(&url).multipart(form).send().await.expect("Failed to send request");

    // Should return 400 Bad Request
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_missing_media_field() {
    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping end-to-end tests: local TCP bind not permitted");
        return;
    };

    // Send request with only config, no media
    let form = multipart::Form::new().text("config", "steps:\n  - kind: passthrough");

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/process");

    let response = client.post(&url).multipart(form).send().await.expect("Failed to send request");

    // Should return 400 Bad Request
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
