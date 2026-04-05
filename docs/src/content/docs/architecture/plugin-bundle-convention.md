---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Plugin Bundle Convention
description: Proposed directory layout for plugin bundles
---

> [!NOTE]
> This is a **design proposal** for unifying the plugin directory layout.
> The migration is incremental — existing bare `.so` plugins continue to work.
> See [Issue #254](https://github.com/streamer45/streamkit/issues/254) for
> discussion.

## Motivation

StreamKit currently has two plugin layouts:

| Source | Layout | Example path |
|--------|--------|-------------|
| Local / dev | Bare `.so` in a flat directory | `.plugins/native/libslint.so` |
| Marketplace | Extracted bundle directory | `.plugins/bundles/slint/0.1.0/libslint.so` |

The discrepancy causes problems:

1. **Asset types lost on restart** — marketplace bundles ship `manifest.json`
   but the local loader expects `plugin.yml`. Without a YAML manifest next to
   the library, `collect_plugin_asset_specs` cannot rediscover asset
   declarations after a server restart.
2. **No room for extras** — a bare `.so` cannot carry bundled assets, custom
   UI files, or license metadata.
3. **Two code paths** — `load_native_plugins_from_dir` scans for `.so` files
   while `load_active_plugins_from_dir` reads JSON records.  Both should
   converge on the same convention.

## Proposed convention

Every plugin — whether local or marketplace — should be a **directory**
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

`read_local_plugin_manifest` already searches for:

1. `plugin.yml` / `plugin.yaml` in the same directory as the `.so`
2. `{stem}.plugin.yml` next to the `.so` (flat layout fallback)

No changes to the discovery logic are needed. The key requirement is that
a `plugin.yml` file exists next to the entrypoint library.

## Current state & incremental steps

### Already done

- **Marketplace bundles write `plugin.yml` on install** — during
  `handle_bundle_install`, the marketplace installer now serializes the
  verified manifest as `plugin.yml` next to the entrypoint. This ensures
  `collect_plugin_asset_specs` finds asset declarations on restart.

- **`just copy-plugins-native` copies manifests** — when copying built
  plugins from `target/` to `.plugins/native/`, the recipe also copies
  `plugin.yml` as `{stem}.plugin.yml` alongside the `.so` file.

### Future steps (not yet implemented)

1. **Local plugin directory layout** — allow placing local plugins as
   directories under `.plugins/native/<plugin-id>/` instead of bare `.so`
   files.  `load_native_plugins_from_dir` would check for directories
   containing a `plugin.yml` before falling back to bare `.so` scanning.

2. **Unified loader** — merge `load_native_plugins_from_dir` and
   `load_active_plugins_from_dir` into a single pass that handles both
   directory-based plugins (with `plugin.yml`) and legacy bare `.so` files.

3. **Bundled system assets** — when a plugin bundle includes a `samples/`
   directory, the asset registry should resolve `system_dir` relative to
   the bundle root.  This allows marketplace plugins to ship default
   assets (e.g. example `.slint` files) without requiring separate
   downloads.

4. **Custom UI** — plugins could ship frontend components (e.g.
   `ui/config-panel.jsx`) that the dashboard loads at runtime for
   node-specific configuration UIs.

## Migration path

The migration is **backward-compatible**:

- Bare `.so` files continue to work.  `load_native_plugins_from_dir`
  keeps its current scanning logic as a fallback.
- Marketplace bundles already ARE directories — they just need a
  `plugin.yml` written during installation (now done).
- The `{stem}.plugin.yml` naming convention bridges the flat layout used
  by `just copy-plugins-native`.

No breaking changes are required.  New features (directory-based local
plugins, bundled assets) are additive.
