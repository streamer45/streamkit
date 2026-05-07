---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Authentication
description: Built-in JWT auth for the API, Web UI, and MoQ/WebTransport
---

StreamKit ships with built-in JWT authentication for:

- **HTTP API** (`/api/*`)
- **WebSocket control plane**
- **MoQ/WebTransport** (via a dedicated MoQ token)

## Modes

Configure built-in auth under `[auth]`:

- `auto` (default): **disabled** on loopback binds (e.g. `127.0.0.1`), **enabled** on non-loopback binds (e.g. `0.0.0.0`)
- `enabled`: always require auth
- `disabled`: never require auth (not recommended outside localhost)

```toml
[auth]
mode = "auto" # auto | enabled | disabled
```

## Bootstrap admin token

When auth is enabled for the first time, StreamKit generates a **bootstrap admin token** and writes it to:

- `${auth.state_dir}/admin.token` (default: `.streamkit/auth/admin.token`)
- `${auth.state_dir}/auth.jwk` (Ed25519 private key as a JWK, `0600`)
- `${auth.state_dir}/jwks.json` (public JWKS for verifying and key rotation)

StreamKit enforces a “tokens we mint” policy for both API and MoQ tokens: a token is only accepted if its `jti`
exists in `${auth.state_dir}/tokens.json`. If you migrate or restore an instance, persist the entire
`${auth.state_dir}` directory (not just the signing key).

To print it:

```bash
skit auth print-admin-token
```

Rotate the signing key (and mint a new bootstrap token):

```bash
skit auth rotate-key
```

## CLI token minting

If you prefer not to use the Web UI, you can mint tokens directly via the CLI:

```bash
# API token (aud: skit-api)
skit auth mint api --role admin --label "ci" --ttl-secs 3600 --json

# MoQ token (aud: skit-moq)
# - empty string in --subscribe/--publish means "allow all"
skit auth mint moq --root /session/<id> --subscribe input --publish output --ttl-secs 3600 --json
```

`skit auth mint ...` uses the running server’s HTTP API, and authenticates using `--token` / `--token-file`
(or `${auth.state_dir}/admin.token` if readable on the host).

### JWKS endpoint (public)

When auth is enabled, StreamKit serves the public JWKS at:

- `/.well-known/jwks.json`

Verifier-only services (future control/media nodes, gateways, etc.) can use this to validate StreamKit-issued JWTs without having access to the private signing key.

### Docker note

In the official Docker images, `skit` runs from `/opt/streamkit`, so the default token path is:

- `/opt/streamkit/.streamkit/auth/admin.token`

For persistence across restarts, mount a volume for `/opt/streamkit/.streamkit` (or set `[auth].state_dir` to a mounted path).

## Web UI login (browser)

When auth is enabled:

1. Open the Web UI.
2. You’ll be redirected to `/login`.
3. Paste an API token (e.g. the bootstrap admin token).

StreamKit stores the session as an **HttpOnly cookie** (default name: `skit_session`), so the browser does not need to keep tokens in localStorage.

## Token management UI (admin)

When signed in as `admin`, open **Admin → Access Tokens** (`/admin/tokens`) to:

- Mint additional API tokens (`admin` / `user` / `viewer`) with an optional label + TTL
- Mint MoQ/WebTransport tokens scoped by `root` and publish/subscribe permissions
- List and revoke previously minted tokens

When built-in auth is disabled (loopback default), this UI is shown read-only (token minting is not needed).

## API usage (non-browser clients)

Send the token as a bearer header:

```bash
curl -H "Authorization: Bearer $SKIT_TOKEN" http://127.0.0.1:4545/api/v1/auth/me
```

Admin instances can mint additional (time-bound) tokens via:

- `POST /api/v1/auth/tokens` (API tokens)
- `POST /api/v1/auth/moq-tokens` (MoQ tokens)
- `DELETE /api/v1/auth/tokens/{jti}` (revoke)

## MoQ/WebTransport tokens

MoQ/WebTransport streaming uses a separate JWT audience (`skit-moq`), passed as a `?jwt=` query parameter on the gateway URL. The Web UI handles this automatically; you only need MoQ tokens if you're building a custom MoQ client.

Create one via `POST /api/v1/auth/moq-tokens` (or the CLI: `skit auth mint moq ...`) and connect with:

`https://<host>:<port>/moq?...&jwt=<token>`

> [!TIP]
> The mint endpoint returns a `url_template` field. If `[server].moq_gateway_url` is configured, it's a full absolute URL with `?jwt=` appended; otherwise it's a relative path you append to your gateway base URL.

## CORS + cookies

If you use cookie auth from a browser, CORS must allow credentials. With auth enabled, `server.cors.allowed_origins = ["*"]` is rejected; configure explicit origins instead.

When auth is disabled, `allowed_origins = ["*"]` is allowed (and the server reflects the request `Origin` so credentialed browser requests work), but it is not recommended outside local development.

## Reverse proxy deployments

You can still run StreamKit behind a reverse proxy for TLS, firewalling, rate limiting, etc.

If you prefer **external authentication** instead of StreamKit’s built-in auth, set `auth.mode = "disabled"` and configure a trusted role header (`[permissions].role_header`) that your proxy sets after authenticating the caller. See [Authorization & Roles](/guides/authorization/).
