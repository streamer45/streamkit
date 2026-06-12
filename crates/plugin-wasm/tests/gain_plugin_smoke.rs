// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! End-to-end smoke test driving the prebuilt gain example plugin through the
//! WASI 0.3 async host. Ignored by default because it requires the example
//! component to be built first:
//!
//! ```sh
//! cargo build --release --target wasm32-wasip2 \
//!     --manifest-path examples/plugins/gain-wasm-rust/Cargo.toml
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use streamkit_core::node::{OutputRouting, OutputSender};
use streamkit_core::types::{AudioFrame, Packet};
use streamkit_core::{NodeContext, PipelineMode};
use streamkit_plugin_wasm::{PluginRuntime, PluginRuntimeConfig};
use tokio::sync::mpsc;

fn example_plugin_path(relative: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins").join(relative);
    assert!(path.exists(), "{} not found; build the example first", path.display());
    path
}

async fn run_gain_plugin(path: PathBuf, expected_kind: &str) {
    let runtime = PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime");
    let plugin = runtime.load_plugin(&path).expect("load plugin");
    assert_eq!(plugin.metadata().kind, expected_kind);

    let node = plugin
        .create_node(Some(&serde_json::json!({"gain_db": 6.0206})))
        .expect("create node");

    let (input_tx, input_rx) = mpsc::channel(8);
    let (state_tx, _state_rx) = mpsc::channel(32);
    let (_control_tx, control_rx) = mpsc::channel(8);
    let (routed_tx, mut routed_rx) = mpsc::channel(64);

    let mut inputs = HashMap::new();
    inputs.insert("in".to_string(), input_rx);

    let context = NodeContext {
        inputs,
        input_types: HashMap::new(),
        control_rx,
        output_sender: OutputSender::new("gain".to_string(), OutputRouting::Routed(routed_tx)),
        batch_size: 1,
        state_tx,
        stats_tx: None,
        telemetry_tx: None,
        session_id: None,
        cancellation_token: None,
        pin_management_rx: None,
        audio_pool: None,
        video_pool: None,
        pipeline_mode: PipelineMode::Oneshot,
        view_data_tx: None,
        engine_control_tx: None,
        asset_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let handle = tokio::spawn(async move { node.run(context).await });

    input_tx
        .send(Packet::Audio(AudioFrame::new(48000, 1, vec![0.5f32; 4])))
        .await
        .expect("send input");
    drop(input_tx);

    let (_node, pin, packet) =
        tokio::time::timeout(std::time::Duration::from_secs(10), routed_rx.recv())
            .await
            .expect("timed out waiting for plugin output")
            .expect("output channel closed without packet");
    assert_eq!(&*pin, "out");

    match packet {
        Packet::Audio(frame) => {
            for sample in frame.samples.iter() {
                assert!((sample - 1.0).abs() < 1e-3, "expected ~2x gain, got {sample}");
            }
        },
        other => panic!("unexpected packet: {other:?}"),
    }

    handle.await.expect("join").expect("node run");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the gain example component to be prebuilt"]
async fn rust_gain_plugin_processes_audio_through_async_host() {
    run_gain_plugin(
        example_plugin_path("gain-wasm-rust/target/wasm32-wasip2/release/gain_plugin.wasm"),
        "gain_filter_rust",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the gain example component to be prebuilt"]
async fn c_gain_plugin_processes_audio_through_async_host() {
    run_gain_plugin(
        example_plugin_path("gain-wasm-c/build/gain_plugin_c.wasm"),
        "gain_filter_c",
    )
    .await;
}
