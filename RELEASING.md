<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Releasing StreamKit

This repo ships:

- Server binary: `skit` (crate: `streamkit-server`)
- Client CLI: `skit-cli` (crate: `streamkit-client`)
- Optional crates for other developers (crates.io)

## Marketplace registry + bundles

This release flow publishes signed registry metadata to GitHub Pages and bundle
artifacts to GitHub Releases.

### Setup (one-time)

```bash
# Generate an unencrypted minisign keypair for registry signing.
# Keep the secret key safe and commit the public key.
minisign -G -W -s /tmp/streamkit.key -p docs/public/registry/streamkit.pub
```

Set the GitHub Actions secret `MINISIGN_SECRET_KEY` to the contents of the
secret key file:

```bash
cat /tmp/streamkit.key
```

If `docs/public/registry/streamkit.pub` contains a placeholder, overwrite it
with the generated public key before tagging.

### System dependencies (v1)

- When present, `pocket-tts` requires OpenSSL 3 (`libssl.so.3`, `libcrypto.so.3`).
  - Ubuntu: `libssl3`
- Native plugins expect system `libstdc++` and `libgcc_s`.

### Trigger a release

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

### Marketplace-only release (decoupled)

Use the GitHub Actions workflow `Marketplace Release` with:

- `version`: marketplace version (e.g., `1.2.3`)
- `release_tag` (optional): defaults to `marketplace-v<version>`

This workflow publishes bundle assets to the GitHub Release for `release_tag`
and opens the registry PR without rebuilding the server/UI. Both tag releases
and marketplace-only releases share the same reusable marketplace workflow
(`.github/workflows/marketplace-build.yml`).

Ensure "Allow GitHub Actions to create and approve pull requests" is enabled
in repo settings so the registry PR can be opened automatically.

Registry PR commits are signed by the workflow. Add these secrets:

- `REGISTRY_GPG_PRIVATE_KEY`: ASCII-armored private key for the registry bot
- `REGISTRY_GPG_PASSPHRASE`: passphrase for the private key
- `REGISTRY_GPG_KEY_ID`: GPG key fingerprint for the registry bot

### Verify outputs

- GitHub Release includes `*-bundle.tar.zst` assets.
- Registry metadata is published after merging the registry PR:
  `https://streamkit.dev/registry/index.json`.
- Verify a manifest signature:

```bash
minisign -V -P "$(tail -n 1 docs/public/registry/streamkit.pub)" \
  -m manifest.json \
  -x manifest.minisig
```

## crates.io publishing

### Intended publish set

- `streamkit-core`
- `streamkit-api`
- `streamkit-plugin-sdk-native`
- `streamkit-plugin-sdk-wasm`

Other workspace crates are marked `publish = false` to avoid accidental publication.

### Order

1. Publish `streamkit-core`
2. Publish `streamkit-plugin-sdk-wasm`
3. Publish `streamkit-api`
4. Publish `streamkit-plugin-sdk-native`

### Commands (manual)

```bash
# Authenticate once
cargo login

# Sanity checks
just test
just lint

# Publish in order (use --dry-run first if you want)
cargo publish -p streamkit-core
cargo publish -p streamkit-plugin-sdk-wasm
cargo publish -p streamkit-api
cargo publish -p streamkit-plugin-sdk-native
```

### Notes

- Publishing requires network access to crates.io.
- `streamkit-api` and `streamkit-plugin-sdk-native` depend on `streamkit-core`; if crates.io index propagation is slow, retry the publish step after a minute.
