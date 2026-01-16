---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Security
description: Entry point for securing StreamKit deployments
---

StreamKit is safe-by-default for local development, but production deployments need explicit
security configuration. This page is the entry point and links to the deeper guides.

## Security model at a glance

- Built-in JWT authentication for the API, Web UI, and MoQ/WebTransport
- Role-based access control (RBAC) with least-privileged roles
- Allowlist-based controls for file access and script `fetch()`
- Runtime plugin management gate and plugin sandboxing (WASM)
- Origin validation for browser traffic

## Start here

- [Authentication](/guides/authentication/)
- [Authorization & Roles](/guides/authorization/)
- [Security Configuration](/guides/security-configuration/)
- [Script Node Guide](/guides/script-node/)
- [Writing Plugins](/guides/writing-plugins/)

## Baseline checklist

- Keep auth enabled for any non-loopback bind (`auth.mode = "auto"` or `"enabled"`).
- Set `default_role` to a least-privileged role and review permissions.
- Disable runtime plugin management unless you need it.
- Restrict file read/write paths via `[security]` allowlists.
- Configure `server.cors.allowed_origins` when using browsers.
- Review script fetch allowlists and secrets.
