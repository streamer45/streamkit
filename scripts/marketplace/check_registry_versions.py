#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""
Pre-merge check: detect plugin.yml changes that would break the append-only
registry without a version bump.

For each plugin whose version already has a committed registry manifest
(docs/public/registry/plugins/<id>/<version>/manifest.json), this script
rebuilds the would-be manifest from the current plugin.yml metadata and
compares it against the committed one.  If they differ the script fails,
prompting the developer to bump the version.

Runs without building any artifacts or requiring signing keys, so it is
safe to add to lightweight CI jobs.
"""

import difflib
import json
import pathlib
import sys


def load_yaml(path: pathlib.Path) -> dict:
    try:
        import yaml
    except ImportError as exc:
        raise RuntimeError(
            "PyYAML is required. Install python3-yaml or pip install pyyaml."
        ) from exc
    return yaml.safe_load(path.read_text())


def strip_none(payload: dict) -> dict:
    return {key: value for key, value in payload.items() if value is not None}


def build_manifest_from_plugin(
    plugin: dict, bundle_block: dict | None, variants: list[dict] | None = None
) -> dict:
    """Mirror the manifest shape produced by build_registry.py.

    `variants` are carried over verbatim from the committed manifest; they are
    produced by the build pipeline (not plugin.yml) and must survive the
    append-only comparison untouched.
    """
    manifest = {
        "schema_version": 1,
        "id": plugin["id"],
        "name": plugin.get("name"),
        "version": plugin.get("version"),
        "node_kind": plugin["node_kind"],
        "kind": plugin["kind"],
        "description": plugin.get("description"),
        "license": plugin.get("license"),
        "license_url": plugin.get("license_url"),
        "homepage": plugin.get("homepage"),
        "repository": plugin.get("repo"),
        "entrypoint": plugin["entrypoint"],
        "bundle": bundle_block,
        "compatibility": plugin.get("compatibility"),
        "models": plugin.get("models", []),
        "assets": plugin.get("assets") or None,
    }
    manifest = strip_none(manifest)
    if variants:
        ordered = {}
        for key, value in manifest.items():
            ordered[key] = value
            if key == "bundle":
                ordered["variants"] = variants
        if "variants" not in ordered:
            ordered["variants"] = variants
        manifest = ordered
    return manifest


def main() -> int:
    repo_root = pathlib.Path(__file__).resolve().parents[2]
    registry_dir = repo_root / "docs" / "public" / "registry"
    plugins_root = repo_root / "plugins" / "native"

    if not plugins_root.exists():
        print(f"Missing plugins root: {plugins_root}", file=sys.stderr)
        return 1

    errors = 0

    for plugin_dir in sorted(plugins_root.iterdir()):
        if not plugin_dir.is_dir():
            continue

        metadata_path = plugin_dir / "plugin.yml"
        if not metadata_path.exists():
            metadata_path = plugin_dir / "plugin.yaml"
        if not metadata_path.exists():
            continue

        plugin = load_yaml(metadata_path) or {}
        plugin_id = plugin.get("id")
        version = plugin.get("version")
        if not plugin_id or not version:
            continue

        committed_manifest_path = (
            registry_dir / "plugins" / plugin_id / version / "manifest.json"
        )
        if not committed_manifest_path.exists():
            continue

        committed = json.loads(committed_manifest_path.read_text())
        would_be = build_manifest_from_plugin(
            plugin, committed.get("bundle"), committed.get("variants")
        )

        if committed != would_be:
            errors += 1
            print(
                f"ERROR: {plugin_id}@{version} — plugin.yml has changed but "
                f"version was not bumped.  The registry is append-only; bump "
                f"the version in plugin.yml and Cargo.toml.",
                file=sys.stderr,
            )
            existing_json = json.dumps(committed, indent=2, sort_keys=False)
            would_be_json = json.dumps(would_be, indent=2, sort_keys=False)
            diff = difflib.unified_diff(
                existing_json.splitlines(keepends=True),
                would_be_json.splitlines(keepends=True),
                fromfile="committed",
                tofile="current",
            )
            print("".join(diff), file=sys.stderr)

    if errors:
        print(
            f"\n{errors} plugin(s) would break the append-only registry.  "
            f"Bump the version in each listed plugin's plugin.yml and "
            f"Cargo.toml, then re-run generate_official_plugins.py.",
            file=sys.stderr,
        )
        return 1

    print("Registry version check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
