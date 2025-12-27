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
TAG=v0.1.0 # replace with the latest release tag
curl -fsSL https://raw.githubusercontent.com/streamer45/streamkit/${TAG}/deploy/systemd/install.sh -o streamkit-install.sh
chmod +x streamkit-install.sh

sudo ./streamkit-install.sh --tag ${TAG}
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
- Plugins directory at `/var/lib/streamkit/plugins`

## Configure

- Edit config: `/etc/streamkit/skit.toml`
- Optional env overrides: `/etc/streamkit/streamkit.env`
- View logs: `journalctl -u streamkit -f`

By default the installed config binds to `127.0.0.1:4545`. If you want to expose StreamKit on the network, update `server.address` (and consider putting it behind a reverse proxy).

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
