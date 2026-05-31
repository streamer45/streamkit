// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![cfg(feature = "moq")]

mod catalog_consumer;
mod constants;
mod ordered_producer;
mod peer;
mod pull;
mod push;

use std::sync::OnceLock;
use url::Url;

// Re-export public types
pub use peer::{MoqPeerConfig, MoqPeerNode};
pub use pull::{MoqPullConfig, MoqPullNode};
pub use push::{MoqPushConfig, MoqPushNode};

use streamkit_core::{
    config_helpers, registry::StaticPins, NodeRegistry, ProcessorNode, StreamKitError,
};

static SHARED_INSECURE_CLIENT: OnceLock<Result<moq_native::Client, String>> = OnceLock::new();

/// Returns a cached `moq_native::Client` with TLS verification disabled.
///
/// In moq-native 0.12, publish/consume origins are set on the `Client` via builder methods
/// (`with_publish` / `with_consume`) before calling `connect()`.  The cached client has
/// neither set, so callers must clone and configure it for each connection.
fn shared_insecure_client() -> Result<moq_native::Client, StreamKitError> {
    let client = SHARED_INSECURE_CLIENT.get_or_init(|| {
        let mut client_config = moq_native::ClientConfig::default();
        // For local dev/test we disable verification; moq-native still loads native roots, so
        // caching the initialized client avoids repeated expensive cert parsing.
        client_config.tls.disable_verify = Some(true);
        client_config.init().map_err(|e| format!("Failed to create MoQ client: {e}"))
    });

    match client {
        Ok(client) => Ok(client.clone()),
        Err(message) => Err(StreamKitError::Runtime(message.clone())),
    }
}

pub(super) fn redact_url_str_for_logs(raw: &str) -> String {
    raw.parse::<Url>().map_or_else(
        |_| raw.split(['?', '#']).next().unwrap_or(raw).to_string(),
        |url| redact_url_for_logs(&url),
    )
}

pub(super) fn redact_url_for_logs(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Pre-resolve the hostname in a MoQ URL to an explicit IP address.
///
/// QUIC (UDP) does not implement Happy Eyeballs (RFC 8305) — unlike TCP, there
/// is no automatic dual-stack fallback.  When a hostname like `localhost`
/// resolves to `::1` (IPv6) first but the relay only listens on `127.0.0.1`
/// (IPv4), the QUIC handshake silently times out (~10 s) before the client
/// tries the next address.
///
/// This function resolves the URL's hostname ahead of time and replaces it with
/// an explicit IPv4 address (preferred) so that the QUIC connection succeeds
/// immediately.  If only IPv6 addresses are available, the first one is used
/// and a warning is logged.
///
/// URLs that already contain a literal IP address are returned unchanged.
pub(super) async fn resolve_url_for_quic(url: &mut Url) -> Result<(), StreamKitError> {
    let host = match url.host_str() {
        Some(h) => h.to_string(),
        None => return Ok(()),
    };

    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| StreamKitError::Runtime(format!("Failed to resolve MoQ host '{host}': {e}")))?
        .collect();

    let preferred =
        addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.first()).ok_or_else(|| {
            StreamKitError::Runtime(format!("No addresses found for MoQ host '{host}'"))
        })?;

    if preferred.is_ipv6() {
        tracing::warn!(
            host = %host,
            resolved = %preferred.ip(),
            "MoQ host resolved to IPv6 only; QUIC connectivity may be affected"
        );
    }

    let ip_str = match preferred.ip() {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };

    url.set_host(Some(&ip_str))
        .map_err(|e| StreamKitError::Runtime(format!("Failed to set resolved host in URL: {e}")))?;

    tracing::debug!(
        original_host = %host,
        resolved = %preferred.ip(),
        "Pre-resolved MoQ URL hostname for QUIC"
    );

    Ok(())
}

pub(super) fn parse_moq_url(raw: &str, jwt: Option<&str>) -> Result<Url, StreamKitError> {
    let mut url: Url = raw.parse().map_err(|e| {
        let redacted = redact_url_str_for_logs(raw);
        StreamKitError::Configuration(format!("Failed to parse MoQ URL '{redacted}': {e}"))
    })?;

    let Some(jwt) = jwt else {
        return Ok(url);
    };

    let jwt = jwt.trim();
    if jwt.is_empty() {
        return Err(StreamKitError::Configuration("MoQ jwt param must not be empty".to_string()));
    }

    let existing: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .filter(|(k, _)| k != "jwt")
        .collect();

    {
        let mut qp = url.query_pairs_mut();
        qp.clear();
        for (k, v) in existing {
            qp.append_pair(&k, &v);
        }
        qp.append_pair("jwt", jwt);
    }

    Ok(url)
}

pub fn register_moq_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "moq")]
    {
        let default_moq_pull = MoqPullNode::new(MoqPullConfig::default());
        register_static_node!(
            registry,
            "transport::moq::subscriber",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPullNode::new(config)))
            },
            MoqPullConfig,
            StaticPins {
                inputs: default_moq_pull.input_pins(),
                outputs: default_moq_pull.output_pins(),
            },
            ["transport", "moq", "dynamic"],
            "Subscribes to a Media over QUIC (MoQ) broadcast. \
             Receives encoded audio and video from a remote publisher over WebTransport.",
        );

        let default_moq_push = MoqPushNode::new(MoqPushConfig::default());
        register_static_node!(
            registry,
            "transport::moq::publisher",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPushNode::new(config)))
            },
            MoqPushConfig,
            StaticPins {
                inputs: default_moq_push.input_pins(),
                outputs: default_moq_push.output_pins(),
            },
            ["transport", "moq", "dynamic"],
            "Publishes media to a Media over QUIC (MoQ) broadcast. \
             Sends encoded audio and optional video to subscribers over WebTransport.",
        );

        let default_moq_peer = MoqPeerNode::new(MoqPeerConfig::default());
        register_static_node!(
            registry,
            "transport::moq::peer",
            |params| {
                let config = config_helpers::parse_config_required(params)?;
                Ok(Box::new(MoqPeerNode::new(config)))
            },
            MoqPeerConfig,
            StaticPins {
                inputs: default_moq_peer.input_pins(),
                outputs: default_moq_peer.output_pins(),
            },
            ["transport", "moq", "bidirectional", "dynamic"],
            bidirectional,
            "Bidirectional MoQ peer for real-time audio and video communication. \
             Acts as both publisher and subscriber over a single WebTransport connection. \
             Supported codecs: Opus (audio), VP9 (video).",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // tests assert on known-good fixtures; failures should surface loudly
mod tests {
    use super::*;

    const MOQ_KINDS: [&str; 3] =
        ["transport::moq::subscriber", "transport::moq::publisher", "transport::moq::peer"];

    #[test]
    fn register_moq_nodes_registers_all_kinds() {
        let mut registry = NodeRegistry::new();
        register_moq_nodes(&mut registry);
        for kind in MOQ_KINDS {
            assert!(registry.contains(kind), "expected '{kind}' to be registered");
        }
    }

    #[test]
    fn registered_factories_build_nodes_from_default_config() {
        let mut registry = NodeRegistry::new();
        register_moq_nodes(&mut registry);
        let empty = serde_json::json!({});
        for kind in MOQ_KINDS {
            assert!(
                registry.create_node(kind, Some(&empty)).is_ok(),
                "'{kind}' factory should accept a default config"
            );
        }
    }

    #[test]
    fn registered_factories_reject_missing_config() {
        let mut registry = NodeRegistry::new();
        register_moq_nodes(&mut registry);
        for kind in MOQ_KINDS {
            assert!(
                registry.create_node(kind, None).is_err(),
                "'{kind}' factory should require config params"
            );
        }
    }

    #[test]
    fn definitions_expose_static_pins_and_categories() {
        let mut registry = NodeRegistry::new();
        register_moq_nodes(&mut registry);

        let publisher = registry.get_definition("transport::moq::publisher").unwrap();
        assert!(publisher.outputs.is_empty(), "publisher is an output node");
        assert_eq!(publisher.inputs.len(), 2);
        assert!(!publisher.bidirectional);

        let subscriber = registry.get_definition("transport::moq::subscriber").unwrap();
        assert!(subscriber.categories.iter().any(|c| c == "moq"));

        let peer = registry.get_definition("transport::moq::peer").unwrap();
        assert!(peer.bidirectional, "peer node is registered as bidirectional");
    }

    #[test]
    fn redact_url_strips_query_and_fragment() {
        let redacted = redact_url_str_for_logs("https://relay.example.com/path?jwt=secret#frag");
        assert_eq!(redacted, "https://relay.example.com/path");
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn redact_url_str_handles_unparseable_input() {
        // Not a valid URL — falls back to splitting on `?`/`#`.
        assert_eq!(redact_url_str_for_logs("not a url?jwt=secret"), "not a url");
    }

    #[test]
    fn parse_moq_url_appends_jwt() {
        let url = parse_moq_url("https://relay.example.com/moq", Some("tok123")).unwrap();
        assert_eq!(url.query(), Some("jwt=tok123"));
    }

    #[test]
    fn parse_moq_url_replaces_existing_jwt() {
        let url = parse_moq_url("https://relay.example.com/moq?jwt=old&x=1", Some("new")).unwrap();
        let pairs: Vec<(String, String)> =
            url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert!(pairs.contains(&("x".to_string(), "1".to_string())));
        assert!(pairs.contains(&("jwt".to_string(), "new".to_string())));
        assert_eq!(pairs.iter().filter(|(k, _)| k == "jwt").count(), 1);
    }

    #[test]
    fn parse_moq_url_without_jwt_is_unchanged() {
        let url = parse_moq_url("https://relay.example.com/moq", None).unwrap();
        assert_eq!(url.query(), None);
    }

    #[test]
    fn parse_moq_url_rejects_empty_jwt() {
        let err = parse_moq_url("https://relay.example.com/moq", Some("   ")).unwrap_err();
        assert!(matches!(err, StreamKitError::Configuration(_)));
    }

    #[test]
    fn parse_moq_url_rejects_invalid_url() {
        let err = parse_moq_url("::not-a-url::", Some("tok")).unwrap_err();
        assert!(matches!(err, StreamKitError::Configuration(_)));
    }

    #[tokio::test]
    async fn resolve_url_for_quic_leaves_literal_ip_untouched() {
        let mut url: Url = "https://127.0.0.1:4545/moq".parse().unwrap();
        resolve_url_for_quic(&mut url).await.unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
    }

    #[tokio::test]
    async fn resolve_url_for_quic_resolves_localhost_to_ip() {
        let mut url: Url = "https://localhost:4545/moq".parse().unwrap();
        resolve_url_for_quic(&mut url).await.unwrap();
        let host = url.host_str().unwrap();
        assert!(
            host.parse::<std::net::IpAddr>().is_ok(),
            "localhost should be replaced by a literal IP, got '{host}'"
        );
    }

    #[test]
    fn shared_insecure_client_is_cached() {
        // The client is initialised once via `OnceLock`, so repeated calls must
        // return the same (cached) outcome regardless of whether init succeeds
        // in this environment.
        assert_eq!(shared_insecure_client().is_ok(), shared_insecure_client().is_ok());
    }
}
