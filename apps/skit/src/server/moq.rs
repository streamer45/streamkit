// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::state::AppState;

#[cfg(feature = "moq")]
pub(super) fn start_moq_webtransport_acceptor(
    app_state: &Arc<AppState>,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    use moq_native::{ServerConfig as MoqServerConfig, ServerTlsConfig};

    let gateway = if let Some(gw) = &app_state.moq_gateway {
        Arc::clone(gw)
    } else {
        warn!("MoQ gateway not initialized, skipping WebTransport acceptor");
        return Ok(());
    };

    let auth_state = Arc::clone(&app_state.auth);

    let addr: SocketAddr =
        config.server.moq_address.as_deref().unwrap_or(&config.server.address).parse()?;

    // TLS priority: moq_cert_path/moq_key_path → server cert_path/key_path (when tls=true) → self-signed.
    let moq_cert = config.server.moq_cert_path.as_deref().filter(|s| !s.is_empty());
    let moq_key = config.server.moq_key_path.as_deref().filter(|s| !s.is_empty());

    if moq_cert.is_some() != moq_key.is_some() {
        return Err(format!(
            "Invalid MoQ TLS config: both moq_cert_path and moq_key_path must be set (got cert={:?}, key={:?})",
            config.server.moq_cert_path, config.server.moq_key_path
        ).into());
    }

    let tls = if let (Some(cert), Some(key)) = (moq_cert, moq_key) {
        info!(cert_path = %cert, key_path = %key, "Using MoQ-specific TLS certificates for WebTransport");
        let mut tls = ServerTlsConfig::default();
        tls.cert = vec![std::path::PathBuf::from(cert)];
        tls.key = vec![std::path::PathBuf::from(key)];
        tls
    } else if config.server.tls
        && !config.server.cert_path.is_empty()
        && !config.server.key_path.is_empty()
    {
        info!(
            cert_path = %config.server.cert_path,
            key_path = %config.server.key_path,
            "Using server TLS certificates for MoQ WebTransport"
        );
        let mut tls = ServerTlsConfig::default();
        tls.cert = vec![std::path::PathBuf::from(&config.server.cert_path)];
        tls.key = vec![std::path::PathBuf::from(&config.server.key_path)];
        tls
    } else {
        info!("Auto-generating self-signed certificate for MoQ WebTransport (14-day validity for local development)");
        let mut tls = ServerTlsConfig::default();
        tls.generate = vec!["localhost".to_string()];
        tls
    };

    let mut moq_config = MoqServerConfig::default();
    moq_config.bind = Some(addr.to_string());
    moq_config.tls = tls;

    let moq_public_paths: Arc<[String]> = config
        .auth
        .moq_public_paths
        .iter()
        .filter(|p| {
            if p.is_empty() {
                warn!("Ignoring empty string in moq_public_paths (would bypass all MoQ auth)");
                false
            } else {
                true
            }
        })
        .cloned()
        .collect::<Vec<_>>()
        .into();

    info!(
        address = %addr,
        moq_public_paths = ?moq_public_paths,
        "Starting MoQ WebTransport acceptor on UDP"
    );

    tokio::spawn(async move {
        match moq_config.init() {
            Ok(mut server) => {
                let fingerprints = server
                    .tls_info()
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fingerprints
                    .clone();
                gateway.set_fingerprints(fingerprints.clone()).await;

                for (i, fp) in fingerprints.iter().enumerate() {
                    info!("🔐 MoQ WebTransport certificate fingerprint #{}: {}", i + 1, fp);
                }
                info!("💡 Access fingerprints at: /api/v1/moq/fingerprints (served by the HTTP server)");

                info!("MoQ WebTransport server listening for connections");

                // Accept connections in a loop
                while let Some(request) = server.accept().await {
                    let gateway = Arc::clone(&gateway);
                    let auth_state = Arc::clone(&auth_state);
                    let moq_public_paths = Arc::clone(&moq_public_paths);

                    tokio::spawn(async move {
                        // Extract URL data before consuming the request.
                        // request.url() borrows, so we copy what we need first.
                        let (path, jwt_param) = {
                            let Some(url) = request.url() else {
                                debug!("Received MoQ connection without URL (raw QUIC), ignoring");
                                return;
                            };
                            let path = url.path().to_string();
                            let jwt_param = url
                                .query_pairs()
                                .find(|(k, _)| k == "jwt")
                                .map(|(_, v)| v.to_string());
                            (path, jwt_param)
                        };

                        // SECURITY: Never log the full URL (may contain jwt)
                        debug!(path = %path, "Received MoQ connection request");

                        // Validate MoQ auth if enabled (skipped for paths matching moq_public_paths).
                        // Segment-based: "/moq" matches "/moq" and "/moq/foo" but NOT "/moq2".
                        let is_public = moq_public_paths.iter().any(|prefix| {
                            path == prefix.as_str() || path.starts_with(&format!("{prefix}/"))
                        });
                        let moq_auth = if auth_state.is_enabled() && !is_public {
                            match validate_moq_auth(&auth_state, &path, jwt_param).await {
                                Ok(ctx) => Some(ctx),
                                Err(status) => {
                                    let _ = request.close(status.as_u16()).await;
                                    return;
                                },
                            }
                        } else {
                            None
                        };

                        if let Err(e) =
                            gateway.accept_connection(request, path.clone(), moq_auth).await
                        {
                            warn!(path = %path, error = %e, "Failed to route MoQ connection");
                        }
                    });
                }

                info!("MoQ WebTransport server stopped accepting connections");
            },
            Err(e) => {
                error!(error = %e, "Failed to initialize MoQ WebTransport server");
            },
        }
    });

    Ok(())
}

/// Validates MoQ auth for an incoming connection, returning the auth context on success
/// or the HTTP status code to reject with on failure.
#[cfg(feature = "moq")]
pub(super) async fn validate_moq_auth(
    auth_state: &crate::auth::AuthState,
    path: &str,
    jwt_param: Option<String>,
) -> Result<Arc<dyn streamkit_core::moq_gateway::MoqAuthChecker>, axum::http::StatusCode> {
    let Some(jwt) = jwt_param else {
        warn!(path = %path, "MoQ auth failed: missing jwt parameter");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    };

    let claims = auth_state.validate_moq_token(&jwt).map_err(|e| {
        warn!(path = %path, error = %e, "MoQ JWT validation failed");
        axum::http::StatusCode::UNAUTHORIZED
    })?;

    if claims.aud != crate::auth::AUD_MOQ {
        warn!(path = %path, expected = crate::auth::AUD_MOQ, actual = %claims.aud, "MoQ auth failed: wrong audience");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    let token_hash = crate::auth::hash_token(&jwt);

    // Enforce "tokens we mint" policy (parity with HTTP API auth).
    let metadata_store = auth_state.token_metadata_store().cloned().ok_or_else(|| {
        warn!(path = %path, "MoQ auth failed: token metadata store not available");
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    })?;

    let meta = metadata_store.get(&claims.jti).await.map_err(|e| {
        warn!(path = %path, error = %e, "MoQ auth failed: metadata store error");
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    })?;

    let Some(meta) = meta else {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token not recognized (not minted by this server)");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    };

    if meta.token_hash != token_hash {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token hash mismatch");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if meta.revoked {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if auth_state.is_revoked(&token_hash) {
        warn!(path = %path, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    crate::auth::verify_moq_token(&claims, path)
        .map_err(|e| {
            warn!(path = %path, error = %e, "MoQ path verification failed");
            axum::http::StatusCode::UNAUTHORIZED
        })
        .map(|ctx| Arc::new(ctx) as Arc<dyn streamkit_core::moq_gateway::MoqAuthChecker>)
}
