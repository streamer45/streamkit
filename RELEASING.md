<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Releasing StreamKit

This repo ships:

- Server binary: `skit` (crate: `streamkit-server`)
- Client CLI: `skit-cli` (crate: `streamkit-client`)
- Optional crates for other developers (crates.io)

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
