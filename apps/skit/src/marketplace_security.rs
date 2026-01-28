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
        validated_get_response(client, policy, label, start, registry_origin, bearer_token).await?;
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
mod tests {
    use super::*;
    use anyhow::{bail, Result};

    fn test_policy() -> MarketplaceUrlPolicy {
        MarketplaceUrlPolicy {
            allowed_origins: Vec::new(),
            require_registry_origin: true,
            scheme_policy: MarketplaceSchemePolicy::HttpsOnly,
            host_policy: MarketplaceHostPolicy::PublicOnly,
            resolve_hostnames: false,
        }
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
}
