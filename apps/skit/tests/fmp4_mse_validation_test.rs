// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! End-to-end regression guard for fragmented-MP4 (fMP4) streaming through the
//! `transport::http::mse` node.
//!
//! Boots an embedded `skit` server, starts the official fMP4/H.264 MSE sample
//! pipeline as a dynamic session, fetches the `/mse/{session_id}/video`
//! endpoint and asserts the served bytes are a valid fMP4 stream: an `ftyp`
//! box at offset 4 (init segment) followed by at least one `moof` media
//! segment. This locks in the Safari/iOS-compatible path added for
//! streamkit#633 so a regression to WebM-only detection is caught in CI.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

use std::net::SocketAddr;
use std::path::Path;

use axum::http::StatusCode;
use serde_json::Value;
use streamkit_server::Config;
use tokio::fs;
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout, Duration, Instant};

async fn start_test_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    // Bind must succeed: a silent skip here would turn this regression guard
    // green without exercising a single assertion. CI runs other server tests
    // that bind 127.0.0.1, so a bind failure is a real problem, not an
    // environment quirk to tolerate.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server listener on 127.0.0.1:0");
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        let (app, _state) = streamkit_server::server::create_app(Config::default(), None);
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    (addr, server_handle)
}

/// Walk top-level ISO-BMFF boxes by declared size and collect their four-byte
/// type tags. Mirrors the parsing contract the node relies on, so it rejects
/// the WebM/EBML output the endpoint used to serve unconditionally.
fn iso_bmff_box_types(data: &[u8]) -> Vec<[u8; 4]> {
    let mut types = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let size = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[offset + 4..offset + 8]);
        types.push(tag);

        let advance = match size {
            0 => break, // box extends to end of stream
            1 => {
                if offset + 16 > data.len() {
                    break;
                }
                let largesize = u64::from_be_bytes([
                    data[offset + 8],
                    data[offset + 9],
                    data[offset + 10],
                    data[offset + 11],
                    data[offset + 12],
                    data[offset + 13],
                    data[offset + 14],
                    data[offset + 15],
                ]);
                usize::try_from(largesize).unwrap_or(usize::MAX)
            },
            n => n,
        };

        if advance < 8 {
            break;
        }
        offset += advance;
    }
    types
}

#[tokio::test]
async fn fmp4_mse_pipeline_serves_fragmented_mp4() {
    let _ = tracing_subscriber::fmt::try_init();

    let (addr, _server_handle) = start_test_server().await;

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-server should live under workspace_root/apps/skit");

    let pipeline_yaml =
        fs::read_to_string(repo_root.join("samples/pipelines/dynamic/video_mse_fmp4_h264.yml"))
            .await
            .expect("Failed to read fMP4 MSE sample pipeline");

    let client = reqwest::Client::new();

    let create_resp = timeout(Duration::from_secs(30), async {
        client
            .post(format!("http://{addr}/api/v1/sessions"))
            .json(&serde_json::json!({ "name": "fmp4-mse-e2e", "yaml": pipeline_yaml }))
            .send()
            .await
    })
    .await
    .expect("Session creation timed out")
    .expect("Failed to create session");

    assert_eq!(
        create_resp.status(),
        StatusCode::OK,
        "Session creation failed: {}",
        create_resp.status()
    );

    let body: Value = create_resp.json().await.expect("Invalid session JSON");
    let session_id = body
        .get("session_id")
        .and_then(Value::as_str)
        .expect("Response missing session_id")
        .to_string();

    let mse_url = format!("http://{addr}/mse/{session_id}/video");

    // The encoder/muxer/sink chain starts asynchronously after the session is
    // created; poll until the endpoint serves a 200 with a body.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut content_type = String::new();
    let mut collected: Vec<u8> = Vec::new();

    'outer: while Instant::now() < deadline {
        let Ok(resp) = client.get(&mse_url).send().await else {
            sleep(Duration::from_millis(250)).await;
            continue;
        };

        if resp.status() != StatusCode::OK {
            sleep(Duration::from_millis(250)).await;
            continue;
        }

        content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        let mut resp = resp;
        // Read chunks until we have the init segment plus a media segment, or
        // the per-fetch budget elapses.
        let chunk_deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < chunk_deadline {
            match timeout(Duration::from_secs(5), resp.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    collected.extend_from_slice(&chunk);
                    let types = iso_bmff_box_types(&collected);
                    let has_ftyp = types.iter().any(|t| t == b"ftyp");
                    let has_moof = types.iter().any(|t| t == b"moof");
                    if has_ftyp && has_moof {
                        break 'outer;
                    }
                    if collected.len() > 1_000_000 {
                        break 'outer;
                    }
                },
                // Stream ended or errored; fall back to a fresh fetch.
                Ok(Ok(None) | Err(_)) | Err(_) => break,
            }
        }

        if !collected.is_empty() {
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }

    assert!(
        !collected.is_empty(),
        "MSE endpoint {mse_url} never served any data within the timeout"
    );

    assert!(
        content_type.starts_with("video/mp4"),
        "Expected fMP4 content-type, got '{content_type}'"
    );

    assert!(
        collected.len() >= 8 && &collected[4..8] == b"ftyp",
        "Stream does not begin with an fMP4 'ftyp' box (got bytes {:02x?})",
        &collected[..collected.len().min(16)]
    );

    let types = iso_bmff_box_types(&collected);

    // Every box type must be printable ASCII. If a media segment were
    // truncated (e.g. the bounded-memory guard dropping the tail of the first
    // moof+mdat), the walk would land mid-payload and read garbage box types —
    // so this guards against the corruption a presence-only check would miss.
    assert!(
        types.iter().all(|t| t.iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ')),
        "Box chain misaligned — a segment was truncated/corrupted (boxes: {:?})",
        types.iter().map(|t| String::from_utf8_lossy(t).into_owned()).collect::<Vec<_>>()
    );

    assert!(
        types.iter().any(|t| t == b"moov"),
        "fMP4 init segment is missing the 'moov' box (boxes: {:?})",
        types.iter().map(|t| String::from_utf8_lossy(t).into_owned()).collect::<Vec<_>>()
    );
    assert!(
        types.iter().any(|t| t == b"moof"),
        "No 'moof' media segment served — init/segment split did not work (boxes: {:?})",
        types.iter().map(|t| String::from_utf8_lossy(t).into_owned()).collect::<Vec<_>>()
    );

    println!(
        "✅ fMP4 MSE validation passed: content-type='{content_type}', {} bytes, boxes={:?}",
        collected.len(),
        types.iter().map(|t| String::from_utf8_lossy(t).into_owned()).collect::<Vec<_>>()
    );
}
