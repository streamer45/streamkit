// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! End-to-end checks that resolved pipeline attributes reach the labels of the
//! metrics the engine emits, in both oneshot (`node.execution.duration`) and
//! dynamic (`node.packets.*`) modes.

// Reason: tests use `.expect(...)` to surface helpful panic messages.
#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

use super::super::*;

/// A shared in-memory metrics pipeline installed as the global provider once.
/// Every engine instrument records into it; tests filter by their own unique
/// `node_id`, so concurrent tests don't interfere.
struct Harness {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

fn harness() -> &'static Harness {
    static HARNESS: OnceLock<Harness> = OnceLock::new();
    HARNESS.get_or_init(|| {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        opentelemetry::global::set_meter_provider(provider.clone());
        Harness { provider, exporter }
    })
}

/// Attributes of every data point of `metric_name` across all exported metrics.
fn data_point_attributes(
    metrics: &[ResourceMetrics],
    metric_name: &str,
) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    for rm in metrics {
        for sm in rm.scope_metrics() {
            for m in sm.metrics().filter(|m| m.name() == metric_name) {
                match m.data() {
                    AggregatedMetrics::F64(MetricData::Histogram(h)) => {
                        out.extend(h.data_points().map(|dp| kvs(dp.attributes())));
                    },
                    AggregatedMetrics::U64(MetricData::Sum(s)) => {
                        out.extend(s.data_points().map(|dp| kvs(dp.attributes())));
                    },
                    _ => {},
                }
            }
        }
    }
    out
}

fn kvs<'a>(attrs: impl Iterator<Item = &'a KeyValue>) -> Vec<(String, String)> {
    attrs.map(|kv| (kv.key.to_string(), kv.value.to_string())).collect()
}

fn point_for_node<'a>(
    points: &'a [Vec<(String, String)>],
    node_id: &str,
) -> &'a Vec<(String, String)> {
    points
        .iter()
        .find(|attrs| attrs.contains(&("node_id".to_string(), node_id.to_string())))
        .expect("a data point for the node must exist")
}

struct ImmediateNode;

#[streamkit_core::async_trait]
impl streamkit_core::ProcessorNode for ImmediateNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        Vec::new()
    }
    async fn run(
        self: Box<Self>,
        _ctx: streamkit_core::NodeContext,
    ) -> Result<(), streamkit_core::StreamKitError> {
        Ok(())
    }
}

#[tokio::test]
async fn oneshot_node_execution_duration_carries_pipeline_attribute() {
    let h = harness();
    let node_id = "metric-attr-oneshot-src";

    let mut nodes: HashMap<String, Box<dyn streamkit_core::ProcessorNode>> = HashMap::new();
    nodes.insert(node_id.to_string(), Box::new(ImmediateNode));
    let node_kinds: HashMap<String, String> =
        std::iter::once((node_id.to_string(), "test::immediate".to_string())).collect();

    let attributes = std::sync::Arc::new(crate::ResolvedAttributes {
        pipeline: vec![KeyValue::new("service", "tts")],
        per_node: HashMap::new(),
    });

    let live = graph_builder::wire_and_spawn_graph(
        nodes,
        &[],
        &node_kinds,
        1,
        crate::constants::DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        None,
        None,
        None,
        super::test_asset_root(),
        attributes,
    )
    .await
    .expect("standalone node should wire and spawn");

    let handles: Vec<_> = live.into_values().map(|n| n.task_handle).collect();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), futures::future::join_all(handles))
            .await;

    h.provider.force_flush().expect("force_flush should succeed");
    let metrics = h.exporter.get_finished_metrics().expect("metrics should be exported");
    let points = data_point_attributes(&metrics, "node.execution.duration");
    let attrs = point_for_node(&points, node_id);

    assert!(
        attrs.contains(&("service".to_string(), "tts".to_string())),
        "node.execution.duration for {node_id} should carry service=tts, got {attrs:?}"
    );
}

#[cfg(feature = "dynamic")]
#[tokio::test]
async fn dynamic_node_packets_carry_pipeline_attribute() {
    use graph_builder::LiveNode;
    use streamkit_core::stats::{NodeStats, NodeStatsUpdate};

    let h = harness();
    let mut engine = super::create_test_engine();
    let node_id = "metric-attr-dynamic-node";

    engine.node_attributes = std::sync::Arc::new(crate::ResolvedAttributes {
        pipeline: vec![KeyValue::new("service", "stt")],
        per_node: HashMap::new(),
    });

    let (control_tx, _control_rx) = tokio::sync::mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(node_id.to_string(), LiveNode { control_tx, task_handle });
    engine.node_kinds.insert(node_id.to_string(), "test::node".to_string());

    engine.handle_stats_update(&NodeStatsUpdate {
        node_id: node_id.to_string(),
        stats: NodeStats { received: 7, sent: 4, discarded: 0, errored: 0, duration_secs: 1.0 },
        timestamp: std::time::SystemTime::now(),
    });

    h.provider.force_flush().expect("force_flush should succeed");
    let metrics = h.exporter.get_finished_metrics().expect("metrics should be exported");
    let points = data_point_attributes(&metrics, "test.received");
    let attrs = point_for_node(&points, node_id);

    assert!(
        attrs.contains(&("service".to_string(), "stt".to_string())),
        "node packets counter for {node_id} should carry service=stt, got {attrs:?}"
    );
}
