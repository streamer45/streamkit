// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use anyhow::Result;
use rand::{distr::Alphanumeric, Rng};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tracing::info;

use std::time::Instant;
use tracing::warn;

use crate::load_test::config::LoadTestConfig;
use crate::load_test::metrics::{FinalMetrics, MetricsCollector, OperationResult, OperationType};
use crate::load_test::workers::{
    cleanup_sessions, create_broadcaster_session, oneshot_worker, session_creator_worker,
    session_tuner_worker, DynamicSession,
};

#[allow(clippy::cognitive_complexity)]
pub async fn run_oneshot_scenario(
    config: &LoadTestConfig,
    metrics: MetricsCollector,
    shutdown: tokio_util::sync::CancellationToken,
    // cleanup parameter is not used in oneshot scenario (no sessions created)
    // but kept for API consistency with other scenario functions
    _cleanup: bool,
) -> Result<FinalMetrics> {
    if !config.oneshot.enabled {
        return Err(anyhow::anyhow!("OneShot scenario is not enabled in config"));
    }

    info!("Starting OneShot scenario with {} workers", config.oneshot.concurrency);

    // Spawn worker tasks
    let mut handles = Vec::new();
    for worker_id in 0..config.oneshot.concurrency {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();

        let handle = tokio::spawn(async move {
            oneshot_worker(worker_id, cfg, m, s).await;
        });
        handles.push(handle);
    }

    // Start progress reporter
    let progress_handle = if config.output.real_time_updates {
        let m = metrics.clone();
        let interval = Duration::from_millis(config.output.update_interval_ms);
        let s = shutdown.clone();

        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let snapshot = m.get_snapshot().await;
                        println!("{snapshot}");
                    }
                    () = s.cancelled() => {
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    // Wait for duration (0 = run until shutdown)
    if config.test.duration_secs == 0 {
        shutdown.cancelled().await;
        info!("Shutdown signal received");
    } else {
        tokio::select! {
            () = sleep(Duration::from_secs(config.test.duration_secs)) => {
                info!("Test duration elapsed, shutting down...");
                shutdown.cancel();
            }
            () = shutdown.cancelled() => {
                info!("Shutdown signal received");
            }
        }
    }

    // Give workers a moment to gracefully shut down
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Abort any workers that haven't stopped yet
    for handle in handles {
        handle.abort();
    }

    if let Some(handle) = progress_handle {
        handle.abort();
    }

    // No cleanup needed for oneshot scenario (no sessions created)

    Ok(metrics.finalize().await)
}

#[allow(clippy::cognitive_complexity)]
pub async fn run_dynamic_scenario(
    config: &LoadTestConfig,
    metrics: MetricsCollector,
    shutdown: tokio_util::sync::CancellationToken,
    cleanup: bool,
) -> Result<FinalMetrics> {
    if !config.dynamic.enabled {
        return Err(anyhow::anyhow!("Dynamic scenario is not enabled in config"));
    }

    let run_id = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();
    info!(run_id = %run_id, "Load test run id (broadcast namespace)");

    info!("Starting Dynamic scenario (target: {} sessions)", config.dynamic.session_count);

    // Track all created session IDs for cleanup
    let session_ids = Arc::new(Mutex::new(Vec::new()));

    // Channel for passing created sessions to tuner
    let (session_tx, session_rx) = mpsc::channel::<DynamicSession>(100);

    // Clone for tracking
    let session_ids_tracker = session_ids.clone();
    let (tracking_tx, mut tracking_rx) = mpsc::channel::<String>(100);
    tokio::spawn(async move {
        while let Some(session_id) = tracking_rx.recv().await {
            session_ids_tracker.lock().await.push(session_id);
        }
    });

    // Create broadcaster sessions first (if configured)
    // Broadcasters publish to canonical broadcast names so subscribers can find them
    if let Some(ref broadcaster) = config.dynamic.broadcaster {
        info!("Creating {} broadcaster session(s)...", broadcaster.count);
        for i in 0..broadcaster.count {
            let start = Instant::now();
            match create_broadcaster_session(
                &broadcaster.pipeline,
                &format!("Broadcaster-{}", i + 1),
                &config.server.url,
                &run_id,
            )
            .await
            {
                Ok(session_id) => {
                    info!("Created broadcaster session: {}", session_id);
                    metrics
                        .record(OperationResult {
                            op_type: OperationType::SessionCreate,
                            latency: start.elapsed(),
                            success: true,
                            error: None,
                        })
                        .await;
                    if cleanup {
                        session_ids.lock().await.push(session_id);
                    }
                },
                Err(e) => {
                    warn!("Failed to create broadcaster: {}", e);
                    metrics
                        .record(OperationResult {
                            op_type: OperationType::SessionCreate,
                            latency: start.elapsed(),
                            success: false,
                            error: Some(e.to_string()),
                        })
                        .await;
                },
            }
        }
        // Brief delay to let broadcasters connect to MoQ relay
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Spawn session creator
    let creator_handle = {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();
        let track_tx = if cleanup { Some(tracking_tx) } else { None };
        let run_id = run_id.clone();
        tokio::spawn(async move {
            session_creator_worker(cfg, m, session_tx, s, track_tx, run_id).await;
        })
    };

    // Spawn session tuner
    let tuner_handle = {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();
        tokio::spawn(async move {
            session_tuner_worker(cfg, m, session_rx, s).await;
        })
    };

    // Start progress reporter
    let progress_handle = if config.output.real_time_updates {
        let m = metrics.clone();
        let interval = Duration::from_millis(config.output.update_interval_ms);
        let s = shutdown.clone();

        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let snapshot = m.get_snapshot().await;
                        println!("{snapshot}");
                    }
                    () = s.cancelled() => {
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    // Wait for duration (0 = run until shutdown)
    if config.test.duration_secs == 0 {
        shutdown.cancelled().await;
        info!("Shutdown signal received");
    } else {
        tokio::select! {
            () = sleep(Duration::from_secs(config.test.duration_secs)) => {
                info!("Test duration elapsed, shutting down...");
                shutdown.cancel();
            }
            () = shutdown.cancelled() => {
                info!("Shutdown signal received");
            }
        }
    }

    // Give workers a moment to gracefully shut down
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Abort workers if they haven't stopped
    creator_handle.abort();
    tuner_handle.abort();

    if let Some(handle) = progress_handle {
        handle.abort();
    }

    // Cleanup sessions if requested
    if cleanup {
        let ids = session_ids.lock().await.clone();
        if !ids.is_empty() {
            info!("Cleaning up {} sessions...", ids.len());
            cleanup_sessions(ids, &config.server.url, metrics.clone()).await;
        }
    }

    Ok(metrics.finalize().await)
}

#[allow(clippy::cognitive_complexity)]
pub async fn run_mixed_scenario(
    config: &LoadTestConfig,
    metrics: MetricsCollector,
    shutdown: tokio_util::sync::CancellationToken,
    cleanup: bool,
) -> Result<FinalMetrics> {
    if !config.oneshot.enabled || !config.dynamic.enabled {
        return Err(anyhow::anyhow!(
            "Mixed scenario requires both oneshot and dynamic to be enabled"
        ));
    }

    info!(
        "Starting Mixed scenario ({} oneshot workers, {} sessions)",
        config.oneshot.concurrency, config.dynamic.session_count
    );

    let run_id = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();
    info!(run_id = %run_id, "Load test run id (broadcast namespace)");

    // Track all created session IDs for cleanup
    let session_ids = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();

    // Spawn oneshot workers
    for worker_id in 0..config.oneshot.concurrency {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();

        let handle = tokio::spawn(async move {
            oneshot_worker(worker_id, cfg, m, s).await;
        });
        handles.push(handle);
    }

    // Channel for dynamic sessions
    let (session_tx, session_rx) = mpsc::channel(100);

    // Clone for tracking
    let session_ids_tracker = session_ids.clone();
    let (tracking_tx, mut tracking_rx) = mpsc::channel::<String>(100);
    tokio::spawn(async move {
        while let Some(session_id) = tracking_rx.recv().await {
            session_ids_tracker.lock().await.push(session_id);
        }
    });

    // Create broadcaster sessions first (if configured)
    // Broadcasters publish to canonical broadcast names so subscribers can find them
    if let Some(ref broadcaster) = config.dynamic.broadcaster {
        info!("Creating {} broadcaster session(s)...", broadcaster.count);
        for i in 0..broadcaster.count {
            let start = Instant::now();
            match create_broadcaster_session(
                &broadcaster.pipeline,
                &format!("Broadcaster-{}", i + 1),
                &config.server.url,
                &run_id,
            )
            .await
            {
                Ok(session_id) => {
                    info!("Created broadcaster session: {}", session_id);
                    metrics
                        .record(OperationResult {
                            op_type: OperationType::SessionCreate,
                            latency: start.elapsed(),
                            success: true,
                            error: None,
                        })
                        .await;
                    if cleanup {
                        session_ids.lock().await.push(session_id);
                    }
                },
                Err(e) => {
                    warn!("Failed to create broadcaster: {}", e);
                    metrics
                        .record(OperationResult {
                            op_type: OperationType::SessionCreate,
                            latency: start.elapsed(),
                            success: false,
                            error: Some(e.to_string()),
                        })
                        .await;
                },
            }
        }
        // Brief delay to let broadcasters connect to MoQ relay
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Spawn session creator
    let creator_handle = {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();
        let track_tx = if cleanup { Some(tracking_tx) } else { None };
        let run_id = run_id.clone();
        tokio::spawn(async move {
            session_creator_worker(cfg, m, session_tx, s, track_tx, run_id).await;
        })
    };
    handles.push(creator_handle);

    // Spawn session tuner
    let tuner_handle = {
        let cfg = config.clone();
        let m = metrics.clone();
        let s = shutdown.clone();
        tokio::spawn(async move {
            session_tuner_worker(cfg, m, session_rx, s).await;
        })
    };
    handles.push(tuner_handle);

    // Start progress reporter
    let progress_handle = if config.output.real_time_updates {
        let m = metrics.clone();
        let interval = Duration::from_millis(config.output.update_interval_ms);
        let s = shutdown.clone();

        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let snapshot = m.get_snapshot().await;
                        println!("{snapshot}");
                    }
                    () = s.cancelled() => {
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    // Wait for duration (0 = run until shutdown)
    if config.test.duration_secs == 0 {
        shutdown.cancelled().await;
        info!("Shutdown signal received");
    } else {
        tokio::select! {
            () = sleep(Duration::from_secs(config.test.duration_secs)) => {
                info!("Test duration elapsed, shutting down...");
                shutdown.cancel();
            }
            () = shutdown.cancelled() => {
                info!("Shutdown signal received");
            }
        }
    }

    // Give workers a moment to gracefully shut down
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Abort any workers that haven't stopped yet
    for handle in handles {
        handle.abort();
    }

    if let Some(handle) = progress_handle {
        handle.abort();
    }

    // Cleanup sessions if requested
    if cleanup {
        let ids = session_ids.lock().await.clone();
        if !ids.is_empty() {
            info!("Cleaning up {} sessions...", ids.len());
            cleanup_sessions(ids, &config.server.url, metrics.clone()).await;
        }
    }

    Ok(metrics.finalize().await)
}
