---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: systemd Deployment
description: Install StreamKit from GitHub Releases and run via systemd
---

This install path is a middle-ground between Docker and "build from source": you download a GitHub Release tarball and run `skit` as a native `systemd` service.

## Install

On a systemd-based Linux host:

```bash
export TAG=v0.2.0 # replace with the latest release tag
curl -fsSL "https://raw.githubusercontent.com/streamer45/streamkit/${TAG}/deploy/systemd/install.sh" -o streamkit-install.sh
chmod +x streamkit-install.sh
sudo ./streamkit-install.sh --tag "${TAG}"
```

> [!TIP]
> For convenience (less reproducible), you can install the latest release:
>
> ```bash
> curl -fsSL https://raw.githubusercontent.com/streamer45/streamkit/main/deploy/systemd/install.sh -o streamkit-install.sh
> chmod +x streamkit-install.sh
> sudo ./streamkit-install.sh --latest
> ```

This installs:

- Binaries under `/opt/streamkit` (versioned releases + symlinks)
- Service unit at `/etc/systemd/system/streamkit.service`
- Config at `/etc/streamkit/skit.toml`
- State directory at `/var/lib/streamkit` (auth state and dynamically loaded plugins)
- Plugins directory at `/var/lib/streamkit/plugins`
- Logs directory at `/var/log/streamkit` (created by `LogsDirectory=streamkit`; journald remains the primary log view)

## Configure

- Edit config: `/etc/streamkit/skit.toml`
- Optional env overrides: `/etc/streamkit/streamkit.env`
- View logs: `journalctl -u streamkit -f`
- Inspect the systemd-managed logs directory if file logging is enabled later: `/var/log/streamkit`

By default the installed config binds to `127.0.0.1:4545`. If you want to expose StreamKit on the network, update `server.address` (and consider putting it behind a reverse proxy).

If you bind to a non-loopback address (e.g. `0.0.0.0:4545`), StreamKit enables built-in auth by default (`auth.mode = "auto"`). The bootstrap admin token is written to the auth state directory (recommended: `/var/lib/streamkit/auth/admin.token` for systemd installs).

To print the bootstrap token:

```bash
sudo -u streamkit /opt/streamkit/skit --config /etc/streamkit/skit.toml auth print-admin-token
```

If you're using MoQ/WebTransport, that listener is QUIC/UDP on the **same port as** `[server].address`. A traditional HTTP reverse proxy (nginx/Caddy) will not handle the MoQ traffic natively; plan a QUIC/WebTransport-aware gateway or an L4 load balancer for UDP/QUIC, alongside your normal HTTP reverse proxy for the UI/API.

## Manage the service

```bash
sudo systemctl status streamkit
sudo systemctl restart streamkit
sudo systemctl stop streamkit
```

## Upgrade

Re-run the installer with a newer tag (or `--latest`) and restart:

```bash
sudo ./streamkit-install.sh --latest
sudo systemctl restart streamkit
```

The installer does not overwrite an existing `/etc/streamkit/skit.toml`. If you're upgrading an older systemd install created before the `[auth]` section was added, add this to your existing config so auth state persists under the systemd state directory:

```toml
[auth]
mode = "auto"
state_dir = "/var/lib/streamkit/auth"
```

## Uninstall

To remove StreamKit while preserving configuration and data:

```bash
sudo ./streamkit-install.sh --uninstall
```

To completely remove everything including config, data, service logs, and the streamkit user:

```bash
sudo ./streamkit-install.sh --uninstall --purge
```

> [!NOTE]
> Without `--purge`, the following are preserved for potential reinstallation:
> - `/etc/streamkit/` (configuration)
> - `/var/lib/streamkit/` (auth state, plugins, and data)
> - `/var/log/streamkit/` (systemd-created logs directory)
> - The `streamkit` system user
