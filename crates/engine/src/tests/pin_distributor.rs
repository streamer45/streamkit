// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::dynamic_messages::{ConnectionId, PinConfigMsg};
use crate::dynamic_pin_distributor::PinDistributorActor;
use streamkit_core::types::Packet;
use tokio::sync::mpsc;

#[tokio::test]
async fn pin_distributor_fanout_delivers_to_all_outputs() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
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

    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle).await;
}

#[tokio::test]
async fn pin_distributor_removes_closed_outputs() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
    let actor_handle = tokio::spawn(actor.run());

    let (open_tx, mut open_rx) = mpsc::channel(8);
    let (closed_tx, closed_rx) = mpsc::channel::<Packet>(1);
    drop(closed_rx); // immediately close this downstream

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

    if let Err(e) = data_tx.send(Packet::Text("one".into())).await {
        panic!("failed to send packet 1: {e}");
    }
    if let Err(e) = data_tx.send(Packet::Text("two".into())).await {
        panic!("failed to send packet 2: {e}");
    }

    let Some(pkt1) = open_rx.recv().await else {
        panic!("open output closed unexpectedly (packet 1)");
    };
    match pkt1 {
        Packet::Text(s) => assert_eq!(s.as_ref(), "one"),
        other => panic!("unexpected packet: {other:?}"),
    }
    let Some(pkt2) = open_rx.recv().await else {
        panic!("open output closed unexpectedly (packet 2)");
    };
    match pkt2 {
        Packet::Text(s) => assert_eq!(s.as_ref(), "two"),
        other => panic!("unexpected packet: {other:?}"),
    }

    drop(data_tx);
    drop(config_tx);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle).await;
}

#[tokio::test]
async fn broadcast_distributes_to_three_outputs() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
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
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle).await;
}

#[tokio::test]
async fn dynamic_pin_add_and_remove() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
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
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle).await;
}

#[tokio::test]
async fn best_effort_drops_when_full() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
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
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), actor_handle).await;
}

#[tokio::test]
async fn shutdown_message_stops_actor() {
    let (data_tx, data_rx) = mpsc::channel(8);
    let (config_tx, config_rx) = mpsc::channel(8);

    let actor =
        PinDistributorActor::new(data_rx, config_rx, "node_a".to_string(), "out".to_string());
    let actor_handle = tokio::spawn(actor.run());

    if let Err(e) = config_tx.send(PinConfigMsg::Shutdown).await {
        panic!("failed to send shutdown: {e}");
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), actor_handle).await;
    assert!(result.is_ok(), "actor should finish after Shutdown");

    drop(data_tx);
}
