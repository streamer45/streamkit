// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Reason: tests use `.expect(...)` on infallible-in-test operations (serde
// round-trips, channel sends to a guaranteed-alive consumer) where a panic
// with context is the clearest failure mode.
#![allow(clippy::expect_used)]

use crate::dynamic_messages::{ConnectionId, PinConfigMsg};
use crate::dynamic_pin_distributor::PinDistributorActor;
use std::sync::{Arc, Mutex};
use streamkit_core::types::Packet;
use tokio::sync::mpsc;

struct WarnCollector(Arc<Mutex<Vec<String>>>);

#[allow(clippy::unwrap_used)]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnCollector {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() == tracing::Level::WARN {
            struct Visitor(String);
            impl tracing::field::Visit for Visitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut v = Visitor(String::new());
            event.record(&mut v);
            self.0.lock().unwrap().push(v.0);
        }
    }
}

#[tokio::test]
async fn pin_distributor_fanout_delivers_to_all_outputs() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (out1_tx, mut out1_rx) = mpsc::channel(8);
    let (out2_tx, mut out2_rx) = mpsc::channel(8);

    let id1 = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_b".to_string(),
        "in".to_string(),
    );
    let id2 = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_c".to_string(),
        "in".to_string(),
    );

    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: id1,
            tx: out1_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add connection 1: {e}");
    }
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: id2,
            tx: out2_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add connection 2: {e}");
    }

    if let Err(e) = data_tx.send(Packet::Text("hello".into())).await {
        panic!("failed to send packet to distributor: {e}");
    }

    let Some(out1_pkt) = out1_rx.recv().await else {
        panic!("output 1 channel closed unexpectedly");
    };
    match out1_pkt {
        Packet::Text(s) => assert_eq!(s.as_ref(), "hello"),
        other => panic!("unexpected packet: {other:?}"),
    }
    let Some(out2_pkt) = out2_rx.recv().await else {
        panic!("output 2 channel closed unexpectedly");
    };
    match out2_pkt {
        Packet::Text(s) => assert_eq!(s.as_ref(), "hello"),
        other => panic!("unexpected packet: {other:?}"),
    }

    drop(data_tx);
    drop(config_tx);

    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
#[allow(clippy::unwrap_used)]
async fn pin_distributor_removes_closed_outputs() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let warnings: Arc<Mutex<Vec<String>>> = Arc::default();
    let subscriber = tracing_subscriber::registry().with(WarnCollector(Arc::clone(&warnings)));

    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle =
        tokio::spawn(actor.run().with_subscriber(tracing::Dispatch::new(subscriber)));

    let (open_tx, mut open_rx) = mpsc::channel(8);
    let (closed_tx, closed_rx) = mpsc::channel::<Packet>(1);
    drop(closed_rx);

    let open_id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_open".to_string(),
        "in".to_string(),
    );
    let closed_id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_closed".to_string(),
        "in".to_string(),
    );

    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: open_id,
            tx: open_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add open connection: {e}");
    }
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: closed_id,
            tx: closed_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add closed connection: {e}");
    }

    if let Err(e) = data_tx.send(Packet::Text("trigger_removal".into())).await {
        panic!("failed to send trigger packet: {e}");
    }

    let Some(pkt) = open_rx.recv().await else {
        panic!("open output closed unexpectedly");
    };
    match pkt {
        Packet::Text(s) => assert_eq!(s.as_ref(), "trigger_removal"),
        other => panic!("unexpected packet: {other:?}"),
    }

    tokio::task::yield_now().await;

    let removal_warnings = warnings.lock().unwrap().len();
    assert_eq!(
        removal_warnings, 1,
        "first packet should trigger exactly one closed-output warning"
    );

    for i in 0..5 {
        if let Err(e) = data_tx.send(Packet::Text(format!("after_{i}").into())).await {
            panic!("failed to send follow-up packet {i}: {e}");
        }
        let Some(pkt) = open_rx.recv().await else {
            panic!("open output closed unexpectedly on follow-up {i}");
        };
        match pkt {
            Packet::Text(s) => assert_eq!(s.as_ref(), &format!("after_{i}")),
            other => panic!("unexpected follow-up packet: {other:?}"),
        }
    }

    tokio::task::yield_now().await;

    let total_warnings = warnings.lock().unwrap().len();
    assert_eq!(
        total_warnings, 1,
        "no additional warnings expected after closed output is removed, got {total_warnings}"
    );

    drop(data_tx);
    drop(config_tx);

    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn broadcast_distributes_to_three_outputs() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let mut receivers = Vec::new();
    for i in 0..3 {
        let (tx, rx) = mpsc::channel(8);
        let id = ConnectionId::new(
            "node_a".to_string(),
            "out".to_string(),
            format!("node_{i}"),
            "in".to_string(),
        );
        if let Err(e) = config_tx
            .send(PinConfigMsg::AddConnection {
                id,
                tx,
                mode: crate::dynamic_messages::ConnectionMode::Reliable,
            })
            .await
        {
            panic!("failed to add connection {i}: {e}");
        }
        receivers.push(rx);
    }

    if let Err(e) = data_tx.send(Packet::Text("broadcast".into())).await {
        panic!("failed to send broadcast packet: {e}");
    }

    for rx in &mut receivers {
        let Some(pkt) = rx.recv().await else {
            panic!("receiver channel closed unexpectedly");
        };
        match pkt {
            Packet::Text(s) => assert_eq!(s.as_ref(), "broadcast"),
            other => panic!("unexpected packet: {other:?}"),
        }
    }

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn dynamic_pin_add_and_remove() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (tx1, mut rx1) = mpsc::channel(8);
    let id1 = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_b".to_string(),
        "in".to_string(),
    );

    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: id1.clone(),
            tx: tx1,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add connection: {e}");
    }

    if let Err(e) = data_tx.send(Packet::Text("first".into())).await {
        panic!("failed to send first packet: {e}");
    }
    let Some(pkt) = rx1.recv().await else {
        panic!("rx1 closed unexpectedly");
    };
    match pkt {
        Packet::Text(s) => assert_eq!(s.as_ref(), "first"),
        other => panic!("unexpected packet: {other:?}"),
    }

    if let Err(e) = config_tx.send(PinConfigMsg::RemoveConnection { id: id1 }).await {
        panic!("failed to remove connection: {e}");
    }

    let (tx2, mut rx2) = mpsc::channel(8);
    let id2 = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_c".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: id2,
            tx: tx2,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add second connection: {e}");
    }

    tokio::task::yield_now().await;

    if let Err(e) = data_tx.send(Packet::Text("second".into())).await {
        panic!("failed to send second packet: {e}");
    }
    let Some(pkt) = rx2.recv().await else {
        panic!("rx2 closed unexpectedly");
    };
    match pkt {
        Packet::Text(s) => assert_eq!(s.as_ref(), "second"),
        other => panic!("unexpected packet: {other:?}"),
    }

    assert!(rx1.try_recv().is_err(), "removed connection should not receive new packets");

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn best_effort_drops_when_full() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (tx, mut rx) = mpsc::channel(1);
    let id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "node_slow".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id,
            tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
    {
        panic!("failed to add best-effort connection: {e}");
    }

    for i in 0..5 {
        if let Err(e) = data_tx.send(Packet::Text(format!("pkt-{i}").into())).await {
            panic!("failed to send packet {i}: {e}");
        }
    }

    let mut received = Vec::new();
    while let Ok(pkt) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
    {
        if let Some(pkt) = pkt {
            if let Packet::Text(s) = pkt {
                received.push(s.to_string());
            }
        } else {
            break;
        }
    }

    assert!(!received.is_empty(), "should have received at least one packet");
    assert!(
        received.len() < 5,
        "best-effort should drop packets when the channel is full, got all {}/5",
        received.len()
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn shutdown_message_stops_actor() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    if let Err(e) = config_tx.send(PinConfigMsg::Shutdown).await {
        panic!("failed to send shutdown: {e}");
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), actor_handle).await;
    assert!(result.is_ok(), "actor should finish after Shutdown");

    drop(data_tx);
}

#[tokio::test]
async fn single_reliable_blocks_on_full_until_consumer_drains() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (slow_tx, mut slow_rx) = mpsc::channel(1);
    let id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "slow".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id,
            tx: slow_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add reliable connection: {e}");
    }

    for i in 0..3 {
        if let Err(e) = data_tx.send(Packet::Text(format!("r-{i}").into())).await {
            panic!("failed to send packet {i}: {e}");
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut received = Vec::new();
    while let Ok(Some(pkt)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), slow_rx.recv()).await
    {
        if let Packet::Text(s) = pkt {
            received.push(s.to_string());
        }
    }

    assert_eq!(
        received,
        vec!["r-0".to_string(), "r-1".to_string(), "r-2".to_string()],
        "Reliable mode must not drop packets; backpressure should serialize delivery"
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

// Verifies the module-level "BestEffort keeps the newest packet when
// downstream is congested" contract: with an idle slow consumer the actor
// must drop intermediate packets but, once the consumer reads, it must
// observe the LATEST packet (be-4), not just the first one or some stale
// middle packet. Regression against https://github.com/streamer45/streamkit/issues/488.
#[tokio::test]
async fn single_best_effort_collapses_to_latest_when_consumer_idle() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (slow_tx, mut slow_rx) = mpsc::channel(1);
    let id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "be".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id,
            tx: slow_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
    {
        panic!("failed to add best-effort connection: {e}");
    }

    for i in 0..5 {
        if let Err(e) = data_tx.send(Packet::Text(format!("be-{i}").into())).await {
            panic!("failed to send packet {i}: {e}");
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut received = Vec::new();
    while let Ok(Some(pkt)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), slow_rx.recv()).await
    {
        if let Packet::Text(s) = pkt {
            received.push(s.to_string());
        }
    }

    assert!(!received.is_empty(), "best-effort must still deliver at least one packet");
    assert!(
        received.len() < 5,
        "single-output best-effort should drop intermediate packets under backpressure, got all {}/5",
        received.len()
    );
    assert_eq!(
        received.last().map(String::as_str),
        Some("be-4"),
        "best-effort must converge on the LATEST packet once the consumer drains; received = {received:?}"
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn remove_unknown_connection_is_silent_noop() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let unknown = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "ghost".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx.send(PinConfigMsg::RemoveConnection { id: unknown }).await {
        panic!("failed to send remove for unknown id: {e}");
    }

    let (tx, mut rx) = mpsc::channel(8);
    let known = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "real".to_string(),
        "in".to_string(),
    );
    if let Err(e) = config_tx
        .send(PinConfigMsg::AddConnection {
            id: known,
            tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
    {
        panic!("failed to add known connection: {e}");
    }
    if let Err(e) = data_tx.send(Packet::Text("after-noop".into())).await {
        panic!("failed to send packet: {e}");
    }
    let Some(Packet::Text(s)) =
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await.ok().flatten()
    else {
        panic!("expected to receive packet after no-op removal");
    };
    assert_eq!(s.as_ref(), "after-noop");

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[tokio::test]
async fn packet_with_no_outputs_is_dropped_without_panic() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    if let Err(e) = data_tx.send(Packet::Text("orphan".into())).await {
        panic!("failed to send orphan packet: {e}");
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(!actor_handle.is_finished(), "actor should still be running after dropping a packet");

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

/// Exercises `packet_stats` for every `Packet` variant by pumping one of each
/// through the actor with a single live consumer. The actor's
/// `distribute_packet` runs `packet_stats` on every packet; without these
/// variants, the Audio/Video/Binary/Transcription/Custom branches stay
/// uncovered.
#[tokio::test]
async fn distribute_packet_handles_all_packet_variants() {
    use streamkit_core::types::{
        AudioFrame, CustomEncoding, CustomPacketData, PacketMetadata, PixelFormat,
        TranscriptionData, VideoFrame,
    };

    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (out_tx, mut out_rx) = mpsc::channel(16);
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "node_b".to_string(),
                "in".to_string(),
            ),
            tx: out_tx,
            mode: crate::dynamic_messages::ConnectionMode::Reliable,
        })
        .await
        .expect("add connection");

    let meta = Some(PacketMetadata {
        timestamp_us: Some(0),
        duration_us: Some(20_000),
        sequence: Some(1),
        keyframe: Some(true),
    });

    let mut audio = AudioFrame::new(48_000, 1, vec![0.0; 64]);
    audio.metadata = meta.clone();
    let packets = vec![
        Packet::Audio(audio),
        Packet::Video(
            VideoFrame::with_metadata(4, 4, PixelFormat::Rgba8, vec![0u8; 4 * 4 * 4], meta.clone())
                .expect("video frame"),
        ),
        Packet::Binary {
            data: bytes::Bytes::from_static(b"hello"),
            content_type: Some(std::borrow::Cow::Borrowed("application/octet-stream")),
            metadata: meta.clone(),
        },
        Packet::Text("a-text-packet".into()),
        Packet::Transcription(Arc::new(TranscriptionData {
            text: "hi".to_string(),
            segments: vec![],
            language: Some("en".to_string()),
            metadata: meta.clone(),
        })),
        Packet::Custom(Arc::new(CustomPacketData {
            type_id: "test::custom@1".to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({"v": 1}),
            metadata: meta,
        })),
    ];

    let expected = packets.len();
    for pkt in packets {
        data_tx.send(pkt).await.expect("send packet");
    }

    let mut got = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while got < expected && std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(100), out_rx.recv()).await {
            Ok(Some(_)) => got += 1,
            Ok(None) => break,
            Err(_) => {},
        }
    }
    assert_eq!(got, expected, "expected {expected} packets, got {got}");

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

/// Drops one consumer in a multi-output (>1) fan-out before sending. The
/// `Reliable` path on a closed channel must hit the `TrySendError::Closed`
/// branch and remove that connection while the alive ones still receive
/// the packet.
#[tokio::test]
async fn multi_output_reliable_drops_closed_consumer() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (alive_tx, mut alive_rx) = mpsc::channel(8);
    let (dead_tx, dead_rx) = mpsc::channel::<Packet>(1);

    for (id_tag, tx) in [("alive", alive_tx), ("dead", dead_tx)] {
        config_tx
            .send(PinConfigMsg::AddConnection {
                id: ConnectionId::new(
                    "node_a".to_string(),
                    "out".to_string(),
                    id_tag.to_string(),
                    "in".to_string(),
                ),
                tx,
                mode: crate::dynamic_messages::ConnectionMode::Reliable,
            })
            .await
            .expect("add connection");
    }

    drop(dead_rx);

    data_tx.send(Packet::Text("first".into())).await.expect("send first");

    match tokio::time::timeout(std::time::Duration::from_secs(1), alive_rx.recv()).await {
        Ok(Some(Packet::Text(s))) => assert_eq!(s.as_ref(), "first"),
        other => panic!("alive consumer should receive 'first'; got {other:?}"),
    }

    // Subsequent send: dead connection has been pruned, alive still receives.
    data_tx.send(Packet::Text("second".into())).await.expect("send second");
    match tokio::time::timeout(std::time::Duration::from_secs(1), alive_rx.recv()).await {
        Ok(Some(Packet::Text(s))) => assert_eq!(s.as_ref(), "second"),
        other => panic!("alive consumer should receive 'second'; got {other:?}"),
    }

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

// Multi-output fan-out variant of the #488 contract: a slow best-effort
// consumer must still observe the LATEST packet once it drains, while a
// reliable peer receives every packet. Verifies the flush arm in the
// multi-output path (distribute_packet's fan-out loop).
#[tokio::test]
async fn multi_output_best_effort_collapses_to_latest_when_slow() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (slow_tx, mut slow_rx) = mpsc::channel(1);
    let (fast_tx, mut fast_rx) = mpsc::channel(16);

    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "slow".to_string(),
                "in".to_string(),
            ),
            tx: slow_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add slow");
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "fast".to_string(),
                "in".to_string(),
            ),
            tx: fast_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add fast");

    for i in 0..5 {
        data_tx.send(Packet::Text(format!("mo-{i}").into())).await.expect("send");
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut fast_received = Vec::new();
    while let Ok(Some(Packet::Text(s))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), fast_rx.recv()).await
    {
        fast_received.push(s.to_string());
    }
    assert_eq!(
        fast_received,
        vec!["mo-0", "mo-1", "mo-2", "mo-3", "mo-4"],
        "fast peer with ample capacity should receive every packet in order"
    );

    let mut slow_received = Vec::new();
    while let Ok(Some(Packet::Text(s))) =
        tokio::time::timeout(std::time::Duration::from_millis(200), slow_rx.recv()).await
    {
        slow_received.push(s.to_string());
    }
    assert!(
        !slow_received.is_empty(),
        "slow best-effort peer must still receive at least one packet"
    );
    assert!(
        slow_received.len() < 5,
        "slow best-effort peer should drop intermediate packets, got all {}/5",
        slow_received.len()
    );
    assert_eq!(
        slow_received.last().map(String::as_str),
        Some("mo-4"),
        "slow best-effort peer must converge on the LATEST packet; received = {slow_received:?}"
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

// Once a best-effort flush has delivered the parked packet, subsequent
// packets sent while the consumer is keeping up must flow through
// directly (no stale pending state, no stuck flush). Guards against a
// regression where flush_in_flight is never reset.
#[tokio::test]
async fn best_effort_flush_then_steady_state_delivers_new_packets() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (tx, mut rx) = mpsc::channel(1);
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "be".to_string(),
                "in".to_string(),
            ),
            tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add connection");

    // Phase 1: drive the actor into "flush in flight" state by bursting
    // packets while the consumer is idle.
    for i in 0..3 {
        data_tx.send(Packet::Text(format!("burst-{i}").into())).await.expect("send burst");
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // Drain the burst (first packet + flushed latest).
    let mut drained = Vec::new();
    while let Ok(Some(Packet::Text(s))) =
        tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
    {
        drained.push(s.to_string());
    }
    assert_eq!(
        drained.last().map(String::as_str),
        Some("burst-2"),
        "phase 1 must converge on the latest burst packet; drained = {drained:?}"
    );

    // Phase 2: with the channel quiet, send a single fresh packet. It must
    // be delivered through the direct try_send path — not blocked behind
    // any leftover flush state.
    data_tx.send(Packet::Text("steady".into())).await.expect("send steady");
    match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
        Ok(Some(Packet::Text(s))) => assert_eq!(
            s.as_ref(),
            "steady",
            "steady-state packet should be delivered cleanly after a prior flush"
        ),
        other => panic!("expected 'steady' packet, got {other:?}"),
    }

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

// If the downstream receiver is dropped while a flush reservation is
// pending, the actor must remove the connection cleanly (the
// reserve_owned() future resolves with an error, not a permit) and
// continue serving any other connection.
#[tokio::test]
async fn best_effort_flush_removes_connection_when_receiver_dropped() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let (slow_tx, slow_rx) = mpsc::channel(1);
    let (alive_tx, mut alive_rx) = mpsc::channel(16);

    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "slow".to_string(),
                "in".to_string(),
            ),
            tx: slow_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add slow");
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "alive".to_string(),
                "in".to_string(),
            ),
            tx: alive_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add alive");

    // Burst packets — slow receiver fills, flush reservation goes in flight.
    for i in 0..3 {
        data_tx.send(Packet::Text(format!("p-{i}").into())).await.expect("send");
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // Drop the slow receiver — its pending reservation must resolve with
    // an error, and the actor must remove the connection.
    drop(slow_rx);

    // Send another packet so the actor's loop runs and observes the flush
    // failure; the alive peer must still receive it.
    data_tx.send(Packet::Text("after-drop".into())).await.expect("send");

    let mut alive_received = Vec::new();
    while let Ok(Some(Packet::Text(s))) =
        tokio::time::timeout(std::time::Duration::from_millis(200), alive_rx.recv()).await
    {
        alive_received.push(s.to_string());
    }
    assert!(
        alive_received.contains(&"after-drop".to_string()),
        "alive peer must keep receiving after the slow peer is dropped; received = {alive_received:?}"
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

// Regression for Devin Review on PR #489: the new flush select-arm
// stays enabled while `pending_flushes` is non-empty. If both input
// receivers close while a flush reservation is still in flight against
// a slow-but-alive downstream (channel full, consumer idle), the actor
// must still exit promptly — the BestEffort contract allows dropping
// the parked packet on shutdown.
#[tokio::test]
async fn best_effort_actor_exits_with_pending_flush_when_inputs_close() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    // Keep slow_rx alive so the reservation never resolves; the only way
    // the actor can exit is by giving up on the pending flush.
    let (slow_tx, _slow_rx_alive) = mpsc::channel(1);
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: ConnectionId::new(
                "node_a".to_string(),
                "out".to_string(),
                "be".to_string(),
                "in".to_string(),
            ),
            tx: slow_tx,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add connection");

    // First packet occupies the single slot; second parks + schedules
    // a reserve_owned() future that will remain pending forever (rx idle).
    data_tx.send(Packet::Text("p-0".into())).await.expect("send p-0");
    data_tx.send(Packet::Text("p-1".into())).await.expect("send p-1");

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    drop(data_tx);
    drop(config_tx);

    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s even when a flush reservation is still pending against an idle downstream")
        .expect("actor task panicked");
}

// Regression for Devin Review on PR #489: a best-effort connection
// removed and re-added with the same ConnectionId must not have its new
// pending_best_effort packet hijacked by the OLD connection's
// reservation resolving against the OLD channel. Each scheduled flush
// is fenced by an OutputConnection generation token.
#[tokio::test]
async fn best_effort_flush_does_not_misroute_after_remove_then_readd() {
    let (data_tx, data_rx) = mpsc::channel(16);
    let (config_tx, config_rx) = mpsc::channel(8);
    let actor = PinDistributorActor::new(
        data_rx,
        config_rx,
        "node_a".to_string(),
        "out".to_string(),
        Vec::new(),
    );
    let actor_handle = tokio::spawn(actor.run());

    let id = ConnectionId::new(
        "node_a".to_string(),
        "out".to_string(),
        "be".to_string(),
        "in".to_string(),
    );

    // v1: park a packet, leave the reservation pending against
    // slow_rx_v1 (kept alive so reserve_owned() can later resolve OK).
    let (tx_v1, mut slow_rx_v1) = mpsc::channel(1);
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: id.clone(),
            tx: tx_v1,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add v1");
    data_tx.send(Packet::Text("v1-a".into())).await.expect("send v1-a");
    data_tx.send(Packet::Text("v1-b".into())).await.expect("send v1-b");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Remove and re-add with the same id but a fresh channel.
    config_tx.send(PinConfigMsg::RemoveConnection { id: id.clone() }).await.expect("remove v1");
    let (tx_v2, mut slow_rx_v2) = mpsc::channel(1);
    config_tx
        .send(PinConfigMsg::AddConnection {
            id: id.clone(),
            tx: tx_v2,
            mode: crate::dynamic_messages::ConnectionMode::BestEffort,
        })
        .await
        .expect("add v2");

    // v2: park a packet, scheduling a NEW reservation against tx_v2.
    data_tx.send(Packet::Text("v2-a".into())).await.expect("send v2-a");
    data_tx.send(Packet::Text("v2-b".into())).await.expect("send v2-b");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Drain v1's channel — the OLD reservation now resolves with
    // Ok(permit_for_v1). Without generation fencing the actor would
    // take v2's "v2-b" out of its pending slot and send it through the
    // v1 permit, delivering it to slow_rx_v1.
    let first_v1 = slow_rx_v1.recv().await.expect("v1 should have v1-a");
    if let Packet::Text(s) = first_v1 {
        assert_eq!(s.as_ref(), "v1-a", "v1 channel should hold its original packet");
    } else {
        panic!("expected text packet on v1");
    }

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // v1's channel must NOT receive any packet from the v2 incarnation.
    // Either state (empty-but-alive, or disconnected because the stale
    // permit was dropped releasing the last Sender clone) is acceptable;
    // both indicate no misroute. A misroute would show up as Ok(...).
    match slow_rx_v1.try_recv() {
        Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {},
        Ok(pkt) => panic!("v1 channel was misrouted a packet from v2: {pkt:?}"),
    }

    // v2 should now have v2-a; drain it to let v2's reservation resolve
    // and deliver the parked v2-b.
    let mut v2_received = Vec::new();
    while let Ok(Some(Packet::Text(s))) =
        tokio::time::timeout(std::time::Duration::from_millis(200), slow_rx_v2.recv()).await
    {
        v2_received.push(s.to_string());
    }
    assert_eq!(
        v2_received,
        vec!["v2-a".to_string(), "v2-b".to_string()],
        "v2 must receive both its packets in order"
    );

    drop(data_tx);
    drop(config_tx);
    tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle)
        .await
        .expect("actor must exit within 1s after both senders drop")
        .expect("actor task panicked");
}

#[test]
fn json_byte_len_matches_serialized_length() {
    use crate::dynamic_pin_distributor::json_byte_len;
    use serde_json::json;

    let cases = vec![
        json!(42),
        json!("ascii"),
        json!("héllo 世界"),
        json!(null),
        json!([1, 2, 3, [4, 5]]),
        json!({"name": "n", "nested": {"k": [true, false, null]}}),
        json!(true),
        json!(1.5),
    ];
    for value in cases {
        let serialized = serde_json::to_string(&value).expect("serialize");
        assert_eq!(
            json_byte_len(&value),
            serialized.len(),
            "json_byte_len mismatch for value: {value:?}"
        );
    }
}
