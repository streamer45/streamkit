---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Plugin Bundle Convention
description: Directory layout for plugin bundles
---

## Motivation

StreamKit plugins can come from multiple sources (local builds, marketplace
installs, manual uploads).  A consistent directory layout ensures that the
server can always discover a plugin's manifest — and therefore its asset
type declarations, model requirements, and other metadata — regardless of
how the plugin was installed.

## Convention

Every plugin — whether local or marketplace — is a **directory**
(a.k.a. "bundle") with a well-known internal layout:

```
<plugin-id>/
├── plugin.yml          # manifest (required)
├── <entrypoint>.so     # plugin binary (required)
├── samples/            # bundled system assets (optional)
│   └── <type_id>/
│       └── system/
│           └── example.slint
├── LICENSES/           # license files (optional)
└── ...                 # future: custom UI, config schemas, etc.
```

### `plugin.yml`

The manifest is the single source of truth for:

- Plugin identity (`id`, `version`, `node_kind`, `kind`)
- Entrypoint path (relative to the bundle root)
- Asset type declarations (`assets[]`)
- Model requirements (`models[]`)
- Compatibility constraints

This is the same schema used by `plugins/native/*/plugin.yml` today.
Marketplace manifests (`manifest.json`) contain the same fields with an
additional `bundle` block for download metadata.

### Discovery rules

`read_local_plugin_manifest` searches for:

1. `plugin.yml` / `plugin.yaml` in the same directory as the `.so`
2. `{stem}.plugin.yml` next to the `.so` (flat layout fallback)

The key requirement is that a `plugin.yml` file exists next to the
entrypoint library.

## Unified loader

`load_all_native_plugins` discovers native plugins from three sources,
loaded in priority order:

1. **Active records** (`.plugins/active/*.json`) — marketplace-installed
   bundles whose entrypoints live under `.plugins/bundles/`.
2. **Directory bundles** (`.plugins/native/<id>/`) — local directory
   layout where each subdirectory contains the plugin library and a
   `plugin.yml` manifest.
3. **Bare library files** (`.plugins/native/*.so`) — legacy flat layout
   for backward compatibility.

A plugin kind that was already loaded by an earlier source is skipped so
that marketplace versions always take precedence, followed by directory
bundles, followed by bare files.

## Build tooling

`just copy-plugins-native` copies built plugins into the directory layout:

```
.plugins/native/slint/
├── libslint.so
└── plugin.yml
```

Each plugin gets its own subdirectory under `.plugins/native/`.  The
`plugin.yml` is copied from the plugin's source tree
(`plugins/native/<id>/plugin.yml`).

## Implemented

- **Marketplace bundles write `plugin.yml` on install** — during
  `handle_bundle_install`, the marketplace installer serializes the
  verified manifest as `plugin.yml` next to the entrypoint.  This ensures
  `collect_plugin_asset_specs` finds asset declarations on restart.

- **`just copy-plugins-native` uses directory layout** — when copying
  built plugins from `target/` to `.plugins/native/`, the recipe creates
  a directory per plugin (e.g. `.plugins/native/slint/`) containing both
  the library and `plugin.yml`.

- **Unified loader** — `load_all_native_plugins` merges the former
  `load_active_plugins_from_dir` and `load_native_plugins_from_dir` into
  a single pass that handles active records, directory bundles, and bare
  library files with correct priority ordering.

## Future steps

1. **Bundled system assets** — when a plugin bundle includes a `samples/`
   directory, the asset registry should resolve `system_dir` relative to
   the bundle root.  This allows marketplace plugins to ship default
   assets (e.g. example `.slint` files) without requiring separate
   downloads.

2. **Custom UI** — plugins could ship frontend components (e.g.
   `ui/config-panel.jsx`) that the dashboard loads at runtime for
   node-specific configuration UIs.

## Migration path

The migration is **backward-compatible**:

- Bare `.so` files continue to work.  The unified loader scans for them
  as a fallback after directory bundles.
- Marketplace bundles already ARE directories — they just need a
  `plugin.yml` written during installation (done).
- The `{stem}.plugin.yml` naming convention is still supported by
  `read_local_plugin_manifest` for any remaining flat layouts.

No breaking changes are required.  New features (bundled assets, custom
UI) are additive.
