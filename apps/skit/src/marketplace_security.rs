// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::fmt::Write;
use std::net::IpAddr;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use reqwest::{header::LOCATION, Client, Response, Url};
use tracing::warn;

use crate::config::{MarketplaceHostPolicy, MarketplaceSchemePolicy, PluginConfig};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginKey {
    scheme: String,
    host: String,
    port: u16,
}

#[derive(Clone, Debug)]
pub struct MarketplaceUrlPolicy {
    allowed_origins: Vec<String>,
    require_registry_origin: bool,
    scheme_policy: MarketplaceSchemePolicy,
    host_policy: MarketplaceHostPolicy,
    resolve_hostnames: bool,
}

pub const MAX_MARKETPLACE_REDIRECTS: usize = 5;

impl MarketplaceUrlPolicy {
    pub fn from_config(config: &PluginConfig) -> Self {
        Self {
            allowed_origins: config.marketplace.security.marketplace_url_allowlist.clone(),
            require_registry_origin: config
                .marketplace
                .security
                .marketplace_require_registry_origin,
            scheme_policy: config.marketplace.security.marketplace_scheme_policy,
            host_policy: config.marketplace.security.marketplace_host_policy,
            resolve_hostnames: config.marketplace.security.marketplace_resolve_hostnames,
        }
    }

    /// Validates a marketplace URL against the configured policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is invalid, violates scheme/host restrictions, or fails
    /// same-origin enforcement when required.
    pub async fn validate_url(
        &self,
        label: &str,
        url: &str,
        registry_origin: Option<&OriginKey>,
    ) -> Result<Url> {
        let parsed = Url::parse(url).with_context(|| format!("Invalid {label} '{url}'"))?;
        validate_scheme(label, &parsed, self.scheme_policy)?;
        validate_host(label, &parsed, self.host_policy)?;
        if self.resolve_hostnames {
            validate_resolved_ips(label, &parsed, self.host_policy).await?;
        }

        let origin = origin_key(&parsed)?;
        let origin_display_str = origin_display(&origin);
        let allowlist_key = origin_allowlist_key(&parsed)?;
        let allowlisted = self
            .allowed_origins
            .iter()
            .any(|pattern| origin_matches_pattern(&allowlist_key, pattern));

        if self.require_registry_origin && !allowlisted {
            if let Some(registry_origin) = registry_origin {
                if &origin != registry_origin {
                    return Err(anyhow!(
                        "{label} origin {origin_display_str} does not match registry origin {registry_origin}",
                        registry_origin = origin_display(registry_origin)
                    ));
                }
            }
        }

        Ok(parsed)
    }
}

/// Fetches a URL while validating every redirect hop against the marketplace policy.
///
/// The initial URL must already be validated by the caller.
///
/// # Errors
///
/// Returns an error if a redirect is invalid, exceeds the redirect limit, or the request fails.
pub async fn validated_get_response(
    client: &Client,
    policy: &MarketplaceUrlPolicy,
    label: &str,
    start: &Url,
    registry_origin: Option<&OriginKey>,
    bearer_token: Option<&str>,
    resume_from_byte: Option<u64>,
) -> Result<(Response, Url)> {
    let mut current = start.clone();
    let token_origin = if bearer_token.is_some() { Some(origin_key(start)?) } else { None };

    for redirect_count in 0..=MAX_MARKETPLACE_REDIRECTS {
        let mut request = client.get(current.clone());
        if let (Some(token), Some(expected_origin)) = (bearer_token, token_origin.as_ref()) {
            let current_origin = origin_key(&current)?;
            if &current_origin == expected_origin {
                request = request.bearer_auth(token);
            }
        }
        if let Some(offset) = resume_from_byte {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response =
            request.send().await.with_context(|| format!("Failed to fetch {label} {current}"))?;
        if response.status().is_redirection() {
            if redirect_count == MAX_MARKETPLACE_REDIRECTS {
                return Err(anyhow!(
                    "{label} exceeded redirect limit ({MAX_MARKETPLACE_REDIRECTS})"
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .ok_or_else(|| anyhow!("{label} redirect missing Location header"))?;
            let location = location.to_str().with_context(|| {
                format!("{label} redirect location is not valid UTF-8: {location:?}")
            })?;
            let next = current
                .join(location)
                .with_context(|| format!("Invalid redirect URL '{location}' for {label}"))?;
            let validated = policy.validate_url(label, next.as_str(), registry_origin).await?;
            current = validated;
            continue;
        }

        return Ok((response, current));
    }

    Err(anyhow!("{label} exceeded redirect limit ({MAX_MARKETPLACE_REDIRECTS})"))
}

/// Fetches a URL and returns the bytes after validating redirects.
///
/// The initial URL must already be validated by the caller.
///
/// # Errors
///
/// Returns an error if the request fails, redirects are invalid, or the response cannot be read.
pub async fn validated_get_bytes(
    client: &Client,
    policy: &MarketplaceUrlPolicy,
    label: &str,
    start: &Url,
    registry_origin: Option<&OriginKey>,
    bearer_token: Option<&str>,
) -> Result<Bytes> {
    let (response, final_url) =
        validated_get_response(client, policy, label, start, registry_origin, bearer_token, None)
            .await?;
    let response = response
        .error_for_status()
        .with_context(|| format!("{label} request failed for {final_url}"))?;
    response
        .bytes()
        .await
        .with_context(|| format!("Failed to read {label} response body from {final_url}"))
}

/// Builds an origin key from a URL.
///
/// # Errors
///
/// Returns an error if the URL is missing a host or does not have a known default port.
pub fn origin_key(url: &Url) -> Result<OriginKey> {
    let host = url.host_str().ok_or_else(|| anyhow!("URL is missing host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL scheme '{}' is missing a default port", url.scheme()))?;
    Ok(OriginKey { scheme: url.scheme().to_string(), host: host.to_string(), port })
}

pub fn origin_display(origin: &OriginKey) -> String {
    format!(
        "{scheme}://{host}:{port}",
        scheme = origin.scheme,
        host = origin.host,
        port = origin.port
    )
}

fn origin_allowlist_key(url: &Url) -> Result<String> {
    let host = url.host_str().ok_or_else(|| anyhow!("URL is missing host"))?;
    let mut origin = format!("{scheme}://{host}", scheme = url.scheme());
    if let Some(port) = url.port() {
        let _ = write!(&mut origin, ":{port}");
    }
    Ok(origin)
}

fn origin_matches_pattern(origin: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Some(prefix_without_port) = pattern.strip_suffix(":*") {
        if origin == prefix_without_port {
            return true;
        }
        let Some(rest) = origin.strip_prefix(prefix_without_port) else {
            return false;
        };
        let Some(port_str) = rest.strip_prefix(':') else {
            return false;
        };
        return !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit());
    }

    origin == pattern
}

fn validate_scheme(label: &str, url: &Url, policy: MarketplaceSchemePolicy) -> Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http" => match policy {
            MarketplaceSchemePolicy::AllowHttp => Ok(()),
            MarketplaceSchemePolicy::HttpsOnly => Err(anyhow!("{label} must use https")),
        },
        other => Err(anyhow!("{label} has unsupported scheme '{other}'")),
    }
}

fn validate_host(label: &str, url: &Url, host_policy: MarketplaceHostPolicy) -> Result<()> {
    let host = url.host_str().ok_or_else(|| anyhow!("{label} is missing host"))?;
    if matches!(host_policy, MarketplaceHostPolicy::PublicOnly) {
        let host_lower = host.to_ascii_lowercase();
        let is_local_tld = Path::new(&host_lower)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("local"));
        if host_lower == "localhost" || host_lower.ends_with(".localhost") || is_local_tld {
            return Err(anyhow!("{label} host '{host}' is not allowed"));
        }
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip, host_policy) {
            return Err(anyhow!("{label} host '{host}' is not allowed"));
        }
    }

    Ok(())
}

async fn validate_resolved_ips(
    label: &str,
    url: &Url,
    host_policy: MarketplaceHostPolicy,
) -> Result<()> {
    let host = url.host_str().ok_or_else(|| anyhow!("{label} is missing host"))?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url.port_or_known_default().ok_or_else(|| anyhow!("{label} is missing a port"))?;

    let lookup = tokio::net::lookup_host((host, port)).await;
    let addrs = match lookup {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(err) => {
            warn!(error = %err, host = %host, "Failed to resolve marketplace host");
            return Ok(());
        },
    };

    if addrs.is_empty() {
        warn!(host = %host, "Marketplace host resolved to no addresses");
        return Ok(());
    }

    for addr in addrs {
        if is_blocked_ip(addr.ip(), host_policy) {
            return Err(anyhow!("{label} resolved to blocked address {}", addr.ip()));
        }
    }

    Ok(())
}

const fn is_blocked_ip(ip: IpAddr, host_policy: MarketplaceHostPolicy) -> bool {
    match ip {
        IpAddr::V4(addr) => {
            let is_private = addr.is_private() || addr.is_loopback() || addr.is_link_local();
            if matches!(host_policy, MarketplaceHostPolicy::PublicOnly) && is_private {
                return true;
            }
            addr.is_unspecified() || addr.is_multicast() || addr.is_broadcast()
        },
        IpAddr::V6(addr) => {
            let is_private =
                addr.is_loopback() || addr.is_unicast_link_local() || addr.is_unique_local();
            if matches!(host_policy, MarketplaceHostPolicy::PublicOnly) && is_private {
                return true;
            }
            addr.is_unspecified() || addr.is_multicast()
        },
    }
}

#[cfg(test)]
// Tests use unwrap/expect for concise assertions; panics surface failures loudly.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use anyhow::{bail, Result};
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::get;
    use axum::Router;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    fn test_policy() -> MarketplaceUrlPolicy {
        MarketplaceUrlPolicy {
            allowed_origins: Vec::new(),
            require_registry_origin: true,
            scheme_policy: MarketplaceSchemePolicy::HttpsOnly,
            host_policy: MarketplaceHostPolicy::PublicOnly,
            resolve_hostnames: false,
        }
    }

    fn permissive_policy() -> MarketplaceUrlPolicy {
        MarketplaceUrlPolicy {
            allowed_origins: Vec::new(),
            require_registry_origin: false,
            scheme_policy: MarketplaceSchemePolicy::AllowHttp,
            host_policy: MarketplaceHostPolicy::AllowPrivate,
            resolve_hostnames: false,
        }
    }

    fn no_redirect_client() -> Client {
        Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
    }

    async fn host_resolves(host: &str, port: u16) -> bool {
        tokio::net::lookup_host((host, port)).await.is_ok_and(|mut addrs| addrs.next().is_some())
    }

    async fn spawn_server(
        app: Router,
    ) -> (std::net::SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        (addr, shutdown_tx, handle)
    }

    #[tokio::test]
    async fn rejects_insecure_marketplace_urls() -> Result<()> {
        let policy = test_policy();
        let registry =
            policy.validate_url("registry index", "https://example.com/index.json", None).await?;
        let registry_origin = origin_key(&registry)?;

        match policy
            .validate_url(
                "manifest url",
                "http://example.com/manifest.json",
                Some(&registry_origin),
            )
            .await
        {
            Ok(_) => bail!("expected https rejection"),
            Err(err) => assert!(err.to_string().contains("https")),
        }

        match policy
            .validate_url("manifest url", "https://evil.com/manifest.json", Some(&registry_origin))
            .await
        {
            Ok(_) => bail!("expected origin rejection"),
            Err(err) => assert!(err.to_string().contains("origin")),
        }

        Ok(())
    }

    #[tokio::test]
    async fn validate_url_accepts_https_under_strict_policy() -> Result<()> {
        let policy = test_policy();
        policy.validate_url("index", "https://example.com/index.json", None).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_url_http_follows_scheme_policy() -> Result<()> {
        let strict = test_policy();
        let err = strict
            .validate_url("index", "http://example.com/index.json", None)
            .await
            .expect_err("http must be rejected under HttpsOnly");
        assert!(err.to_string().contains("must use https"));

        let lenient = MarketplaceUrlPolicy {
            scheme_policy: MarketplaceSchemePolicy::AllowHttp,
            ..test_policy()
        };
        lenient.validate_url("index", "http://example.com/index.json", None).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_url_rejects_unknown_scheme() -> Result<()> {
        let policy = test_policy();
        let err = policy
            .validate_url("index", "ftp://example.com/index.json", None)
            .await
            .expect_err("ftp is unsupported");
        assert!(err.to_string().contains("unsupported scheme"));
        Ok(())
    }

    #[tokio::test]
    async fn validate_url_enforces_registry_origin() -> Result<()> {
        let policy = test_policy();
        let registry = policy.validate_url("index", "https://example.com/index.json", None).await?;
        let registry_origin = origin_key(&registry)?;

        policy
            .validate_url("manifest", "https://example.com/manifest.json", Some(&registry_origin))
            .await?;

        let err = policy
            .validate_url("manifest", "https://other.com/manifest.json", Some(&registry_origin))
            .await
            .expect_err("cross-origin must be rejected");
        assert!(err.to_string().contains("does not match registry origin"));
        Ok(())
    }

    #[tokio::test]
    async fn validate_url_allowlist_bypasses_registry_origin() -> Result<()> {
        let policy = MarketplaceUrlPolicy {
            allowed_origins: vec!["https://cdn.example.net".to_string()],
            ..test_policy()
        };
        let registry_origin = origin_key(&Url::parse("https://registry.example.com/index.json")?)?;

        policy
            .validate_url(
                "bundle",
                "https://cdn.example.net/plugin.tar.zst",
                Some(&registry_origin),
            )
            .await?;
        Ok(())
    }

    #[test]
    fn origin_key_requires_host_and_known_port() -> Result<()> {
        let err = origin_key(&Url::parse("foo:bar")?).expect_err("missing host");
        assert!(err.to_string().contains("missing host"));

        let err = origin_key(&Url::parse("foo://example.com/")?)
            .expect_err("unknown scheme has no default port");
        assert!(err.to_string().contains("missing a default port"));

        let origin = origin_key(&Url::parse("https://example.com/x")?)?;
        assert_eq!(origin.port, 443);
        Ok(())
    }

    #[test]
    fn origin_display_renders_scheme_host_port() -> Result<()> {
        let origin = origin_key(&Url::parse("https://example.com:8443/x")?)?;
        assert_eq!(origin_display(&origin), "https://example.com:8443");
        Ok(())
    }

    #[test]
    fn origin_allowlist_key_includes_explicit_port_only() -> Result<()> {
        assert_eq!(
            origin_allowlist_key(&Url::parse("https://example.com:8443/x")?)?,
            "https://example.com:8443"
        );
        assert_eq!(
            origin_allowlist_key(&Url::parse("https://example.com/x")?)?,
            "https://example.com"
        );
        let err = origin_allowlist_key(&Url::parse("foo:bar")?).expect_err("missing host");
        assert!(err.to_string().contains("missing host"));
        Ok(())
    }

    #[test]
    fn origin_matches_pattern_handles_wildcards_and_ports() {
        let cases = [
            ("https://anything.example", "*", true),
            ("http://127.0.0.1:5000", "http://127.0.0.1:*", true),
            ("http://127.0.0.1", "http://127.0.0.1:*", true),
            ("http://127.0.0.1:", "http://127.0.0.1:*", false),
            ("http://127.0.0.1:abc", "http://127.0.0.1:*", false),
            ("http://other:5000", "http://127.0.0.1:*", false),
            ("https://example.com", "https://example.com", true),
            ("https://example.com", "https://other.com", false),
        ];
        for (origin, pattern, expected) in cases {
            assert_eq!(
                origin_matches_pattern(origin, pattern),
                expected,
                "origin={origin} pattern={pattern}"
            );
        }
    }

    #[test]
    fn validate_scheme_matrix() -> Result<()> {
        let https = Url::parse("https://example.com/")?;
        let http = Url::parse("http://example.com/")?;
        let ftp = Url::parse("ftp://example.com/")?;

        assert!(validate_scheme("x", &https, MarketplaceSchemePolicy::HttpsOnly).is_ok());
        assert!(validate_scheme("x", &https, MarketplaceSchemePolicy::AllowHttp).is_ok());
        assert!(validate_scheme("x", &http, MarketplaceSchemePolicy::AllowHttp).is_ok());

        let err = validate_scheme("x", &http, MarketplaceSchemePolicy::HttpsOnly)
            .expect_err("http rejected");
        assert!(err.to_string().contains("must use https"));
        let err = validate_scheme("x", &ftp, MarketplaceSchemePolicy::AllowHttp)
            .expect_err("ftp rejected");
        assert!(err.to_string().contains("unsupported scheme"));
        Ok(())
    }

    #[test]
    fn validate_host_public_only_blocks_local_and_private() -> Result<()> {
        for host in ["localhost", "plugin.localhost", "printer.local", "10.0.0.1", "192.168.1.1"] {
            let url = Url::parse(&format!("https://{host}/x"))?;
            let err = validate_host("x", &url, MarketplaceHostPolicy::PublicOnly)
                .expect_err("blocked host");
            assert!(err.to_string().contains("not allowed"), "host={host}");
        }
        for host in ["example.com", "8.8.8.8"] {
            let url = Url::parse(&format!("https://{host}/x"))?;
            assert!(validate_host("x", &url, MarketplaceHostPolicy::PublicOnly).is_ok());
        }
        Ok(())
    }

    #[test]
    fn validate_host_allow_private_permits_private_but_blocks_unspecified() -> Result<()> {
        let private = Url::parse("https://10.0.0.1/x")?;
        assert!(validate_host("x", &private, MarketplaceHostPolicy::AllowPrivate).is_ok());

        let unspecified = Url::parse("https://0.0.0.0/x")?;
        let err = validate_host("x", &unspecified, MarketplaceHostPolicy::AllowPrivate)
            .expect_err("unspecified always blocked");
        assert!(err.to_string().contains("not allowed"));
        Ok(())
    }

    #[test]
    fn is_blocked_ip_matrix() {
        let v4 = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        let v6 = |segments: [u16; 8]| IpAddr::V6(Ipv6Addr::from(segments));
        // (ip, blocked under PublicOnly, blocked under AllowPrivate)
        let cases = [
            (v4(10, 0, 0, 1), true, false),
            (v4(172, 16, 0, 1), true, false),
            (v4(192, 168, 1, 1), true, false),
            (v4(127, 0, 0, 1), true, false),
            (v4(169, 254, 0, 1), true, false),
            (v4(0, 0, 0, 0), true, true),
            (v4(224, 0, 0, 1), true, true),
            (v4(255, 255, 255, 255), true, true),
            (v6([0, 0, 0, 0, 0, 0, 0, 1]), true, false),
            (v6([0xfd00, 0, 0, 0, 0, 0, 0, 1]), true, false),
            (v6([0xfe80, 0, 0, 0, 0, 0, 0, 1]), true, false),
            (v6([0, 0, 0, 0, 0, 0, 0, 0]), true, true),
            (v6([0xff02, 0, 0, 0, 0, 0, 0, 1]), true, true),
            (v4(8, 8, 8, 8), false, false),
            (v6([0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888]), false, false),
        ];
        for (ip, public_blocked, private_blocked) in cases {
            assert_eq!(
                is_blocked_ip(ip, MarketplaceHostPolicy::PublicOnly),
                public_blocked,
                "PublicOnly {ip}"
            );
            assert_eq!(
                is_blocked_ip(ip, MarketplaceHostPolicy::AllowPrivate),
                private_blocked,
                "AllowPrivate {ip}"
            );
        }
    }

    #[tokio::test]
    async fn validate_resolved_ips_skips_literal_ip() -> Result<()> {
        let url = Url::parse("https://10.0.0.1/x")?;
        validate_resolved_ips("x", &url, MarketplaceHostPolicy::PublicOnly).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_resolved_ips_blocks_hostname_resolving_to_loopback() -> Result<()> {
        // Skip in hermetic environments where `localhost` does not resolve.
        if !host_resolves("localhost", 80).await {
            return Ok(());
        }
        let url = Url::parse("http://localhost:80/x")?;
        let err = validate_resolved_ips("x", &url, MarketplaceHostPolicy::PublicOnly)
            .await
            .expect_err("localhost resolves to a blocked address");
        assert!(err.to_string().contains("resolved to blocked address"));

        validate_resolved_ips("x", &url, MarketplaceHostPolicy::AllowPrivate).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_resolved_ips_tolerates_unresolvable_host() -> Result<()> {
        let url = Url::parse("https://does-not-exist.invalid/x")?;
        validate_resolved_ips("x", &url, MarketplaceHostPolicy::PublicOnly).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_url_resolve_hostnames_gates_dns_check() -> Result<()> {
        // A trailing-dot host ("localhost.") is not caught by validate_host's
        // literal-localhost check, so under PublicOnly only the DNS-resolution
        // guard can reject it -- which makes the resolve_hostnames flag observable.
        if !host_resolves("localhost.", 443).await {
            return Ok(());
        }
        let url = "https://localhost./index.json";

        let resolving = MarketplaceUrlPolicy { resolve_hostnames: true, ..test_policy() };
        let err = resolving
            .validate_url("index", url, None)
            .await
            .expect_err("loopback resolution must be rejected when resolve_hostnames is enabled");
        assert!(err.to_string().contains("resolved to blocked address"));

        let not_resolving = MarketplaceUrlPolicy { resolve_hostnames: false, ..test_policy() };
        not_resolving.validate_url("index", url, None).await?;
        Ok(())
    }

    #[tokio::test]
    async fn validated_get_response_follows_revalidated_redirect() -> Result<()> {
        let app = Router::new()
            .route("/start", get(|| async { Redirect::temporary("/final").into_response() }))
            .route("/final", get(|| async { "redirected-body" }));
        let (addr, shutdown_tx, handle) = spawn_server(app).await;

        let client = no_redirect_client();
        let policy = permissive_policy();
        let start = Url::parse(&format!("http://{addr}/start"))?;
        let (response, final_url) =
            validated_get_response(&client, &policy, "index", &start, None, None, None).await?;

        assert_eq!(final_url.path(), "/final");
        assert!(response.status().is_success());
        assert_eq!(response.text().await?, "redirected-body");

        let _ = shutdown_tx.send(());
        let _ = handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn validated_get_response_rejects_excessive_redirects() -> Result<()> {
        let app = Router::new()
            .route("/loop", get(|| async { Redirect::temporary("/loop").into_response() }));
        let (addr, shutdown_tx, handle) = spawn_server(app).await;

        let client = no_redirect_client();
        let policy = permissive_policy();
        let start = Url::parse(&format!("http://{addr}/loop"))?;
        let err = validated_get_response(&client, &policy, "index", &start, None, None, None)
            .await
            .expect_err("redirect loop must be rejected");
        assert!(err.to_string().contains("exceeded redirect limit"));

        let _ = shutdown_tx.send(());
        let _ = handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn validated_get_response_requires_location_header() -> Result<()> {
        let app = Router::new().route(
            "/noloc",
            get(|| async {
                Response::builder().status(StatusCode::FOUND).body(Body::empty()).unwrap()
            }),
        );
        let (addr, shutdown_tx, handle) = spawn_server(app).await;

        let client = no_redirect_client();
        let policy = permissive_policy();
        let start = Url::parse(&format!("http://{addr}/noloc"))?;
        let err = validated_get_response(&client, &policy, "index", &start, None, None, None)
            .await
            .expect_err("missing Location must be an error");
        assert!(err.to_string().contains("missing Location header"));

        let _ = shutdown_tx.send(());
        let _ = handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn validated_get_response_drops_bearer_token_cross_origin() -> Result<()> {
        let auth_after_redirect = Arc::new(AtomicBool::new(false));
        let dest_flag = auth_after_redirect.clone();
        let app_b = Router::new().route(
            "/b",
            get(move |headers: HeaderMap| {
                let dest_flag = dest_flag.clone();
                async move {
                    dest_flag.store(headers.contains_key(header::AUTHORIZATION), Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        let (addr_b, shutdown_b, handle_b) = spawn_server(app_b).await;

        let auth_on_first_hop = Arc::new(AtomicBool::new(false));
        let origin_flag = auth_on_first_hop.clone();
        let redirect_target = format!("http://{addr_b}/b");
        let app_a = Router::new().route(
            "/a",
            get(move |headers: HeaderMap| {
                let origin_flag = origin_flag.clone();
                let redirect_target = redirect_target.clone();
                async move {
                    origin_flag
                        .store(headers.contains_key(header::AUTHORIZATION), Ordering::SeqCst);
                    Redirect::temporary(&redirect_target).into_response()
                }
            }),
        );
        let (addr_a, shutdown_a, handle_a) = spawn_server(app_a).await;

        let client = no_redirect_client();
        let policy = permissive_policy();
        let start = Url::parse(&format!("http://{addr_a}/a"))?;
        let (response, final_url) =
            validated_get_response(&client, &policy, "bundle", &start, None, Some("secret"), None)
                .await?;

        assert_eq!(final_url.path(), "/b");
        assert!(response.status().is_success());
        assert!(auth_on_first_hop.load(Ordering::SeqCst), "token must be sent to the start origin");
        assert!(
            !auth_after_redirect.load(Ordering::SeqCst),
            "token must be dropped on cross-origin redirect"
        );

        let _ = shutdown_a.send(());
        let _ = shutdown_b.send(());
        let _ = handle_a.await;
        let _ = handle_b.await;
        Ok(())
    }

    #[tokio::test]
    async fn validated_get_bytes_success_and_error_status() -> Result<()> {
        let app = Router::new().route(
            "/ok",
            get(|| async {
                ([(header::CONTENT_TYPE, "application/octet-stream")], Bytes::from_static(b"hello"))
            }),
        );
        let (addr, shutdown_tx, handle) = spawn_server(app).await;

        let client = no_redirect_client();
        let policy = permissive_policy();

        let ok_url = Url::parse(&format!("http://{addr}/ok"))?;
        let bytes = validated_get_bytes(&client, &policy, "index", &ok_url, None, None).await?;
        assert_eq!(bytes.as_ref(), b"hello");

        let missing_url = Url::parse(&format!("http://{addr}/missing"))?;
        let err = validated_get_bytes(&client, &policy, "index", &missing_url, None, None)
            .await
            .expect_err("404 must surface as an error");
        assert!(err.to_string().contains("request failed"));

        let _ = shutdown_tx.send(());
        let _ = handle.await;
        Ok(())
    }
}
