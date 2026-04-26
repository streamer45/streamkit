---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Installing Plugins
description: Install marketplace plugins or upload trusted plugins manually
---

StreamKit supports two install paths:

- Marketplace installs (recommended)
- Manual upload (trusted code only)

## Marketplace prerequisites

Enable marketplace browsing and install gates:

```toml
[plugins]
marketplace_enabled = true
allow_native_marketplace = false # set true only if you trust the registry
registries = ["https://streamkit.dev/registry/index.json"]
trusted_pubkeys = [
  "untrusted comment: minisign public key 81C485A94492F33F\nRWQ/85JEqYXEgX+2kl7Rwd8AcpVjYciSLzvLggzivbGyIrDPjfmcqjYP\n",
]
```

Use `https://streamkit.dev/registry/index.json` for the official registry; the generated `docs/public/registry/index.json` file is what the docs site serves at that URL.

RBAC must allow plugin operations:

- `load_plugins = true` to install
- `delete_plugins = true` to uninstall
- `allowed_plugins` must include the plugin kind (e.g., `plugin::native::whisper` or `plugin::*`)

Optional model download settings:

```toml
[plugins]
models_dir = "/var/lib/streamkit/models"
huggingface_token = "${HF_TOKEN}"
```

> [!NOTE]
> Marketplace installs are blocked when `[plugins].marketplace_enabled = false`. Native marketplace installs
> are blocked unless `[plugins].allow_native_marketplace = true`.

## Marketplace URL security

By default, marketplace URLs must use HTTPS and resolve to public hosts only. Localhost, private,
link-local, multicast, and `.local` hosts are blocked. Same-origin enforcement is optional; set
`marketplace_require_registry_origin = true` for stricter deployments.

`marketplace_url_allowlist` relaxes only the same-origin requirement. It does **not** bypass HTTPS
or host/IP blocking.

If you enable same-origin enforcement and your registry index is on GitHub Pages while bundles are
on GitHub Releases, you must allowlist all hosts in the redirect chain (the installer validates
every hop). Example:

```toml
[plugins]
marketplace_url_allowlist = [
  "https://github.com",
  "https://objects.githubusercontent.com",
  "https://release-assets.githubusercontent.com",
]
```

For local testing, explicitly opt in:

```toml
[plugins]
marketplace_scheme_policy = "allow_http"
marketplace_host_policy = "allow_private"
marketplace_url_allowlist = ["http://127.0.0.1:*"]
```

Optional DNS checks (best-effort):

```toml
[plugins]
marketplace_resolve_hostnames = true
```

> [!NOTE]
> DNS rebinding cannot be fully prevented. Hostname validation happens at request time and DNS
> answers can change afterward.

## Install via the UI

1. Open **Admin → Plugins → Marketplace**.
2. Choose a registry and select a plugin.
3. Verify the signature status and review licenses.
4. Toggle **Download models after install** if available. When a plugin defines multiple models,
   select the ones you want from the checklist.
5. Click **Install** and watch the progress job.

If a model is marked as gated, the server must have a Hugging Face token configured or the job will fail.

## Uninstalling plugins

Use **Admin → Plugins → Installed** to unload a plugin. For marketplace installs, this removes the active
record and bundle on disk (unless you pass `keep_file=true` via the API).

Manual API uninstall:

```bash
curl -X DELETE "http://127.0.0.1:4545/api/v1/plugins/plugin%3A%3Anative%3A%3Again"
```

## Manual upload (trusted only)

Manual upload is disabled by default and should be reserved for trusted environments.

```toml
[plugins]
allow_http_management = true
```

Upload using the UI or:

```bash
curl -F plugin=@libmy_plugin.so http://127.0.0.1:4545/api/v1/plugins
```

Manual uploads are stored under `.plugins/native/` or `.plugins/wasm/` and are loaded on restart.
