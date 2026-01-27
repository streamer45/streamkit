#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

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


def load_toml(path: pathlib.Path) -> dict:
    try:
        import tomllib
    except ImportError:
        try:
            import tomli as tomllib
        except ImportError as exc:
            raise RuntimeError(
                "tomli is required for Python < 3.11. Install python3-tomli or pip install tomli."
            ) from exc
    return tomllib.loads(path.read_text())


def validate_plugin(plugin: dict, plugin_dir: pathlib.Path) -> dict:
    required = ["id", "name", "node_kind", "kind", "entrypoint", "artifact", "description", "license", "version"]
    missing = [key for key in required if not plugin.get(key)]
    if missing:
        raise ValueError(f"{plugin_dir}: missing required fields: {', '.join(missing)}")
    if plugin["id"] != plugin_dir.name:
        raise ValueError(
            f"{plugin_dir}: id '{plugin['id']}' must match directory name"
        )

    # For native plugins, enforce version matches Cargo.toml
    if plugin["kind"] == "native":
        cargo_toml_path = plugin_dir / "Cargo.toml"
        if not cargo_toml_path.exists():
            raise ValueError(f"{plugin_dir}: native plugin missing Cargo.toml")
        cargo_data = load_toml(cargo_toml_path)
        cargo_version = cargo_data.get("package", {}).get("version")
        if not cargo_version:
            raise ValueError(f"{plugin_dir}: Cargo.toml missing [package].version")
        if plugin["version"] != cargo_version:
            raise ValueError(
                f"{plugin_dir}: version mismatch; plugin.yml has '{plugin['version']}' "
                f"but Cargo.toml has '{cargo_version}'. Update both or keep them aligned."
            )

    ordered_keys = [
        "id",
        "name",
        "version",
        "node_kind",
        "kind",
        "entrypoint",
        "artifact",
        "description",
        "license",
        "license_url",
        "homepage",
        "repository",
        "compatibility",
        "models",
    ]
    ordered = {key: plugin[key] for key in ordered_keys if key in plugin}
    for key, value in plugin.items():
        if key not in ordered:
            ordered[key] = value
    return ordered


def main() -> int:
    repo_root = pathlib.Path(__file__).resolve().parents[2]
    plugins_root = repo_root / "plugins" / "native"
    output_path = repo_root / "marketplace" / "official-plugins.json"

    if not plugins_root.exists():
        print(f"Missing plugins root: {plugins_root}", file=sys.stderr)
        return 1

    plugins = []
    errors = []
    for plugin_dir in sorted(plugins_root.iterdir()):
        if not plugin_dir.is_dir():
            continue
        # Search order: plugin.yml, plugin.yaml, then deprecated marketplace.yml/yaml
        metadata_path = None
        is_deprecated = False
        for candidate in ["plugin.yml", "plugin.yaml"]:
            candidate_path = plugin_dir / candidate
            if candidate_path.exists():
                metadata_path = candidate_path
                break
        if not metadata_path:
            for candidate in ["marketplace.yml", "marketplace.yaml"]:
                candidate_path = plugin_dir / candidate
                if candidate_path.exists():
                    metadata_path = candidate_path
                    is_deprecated = True
                    print(
                        f"WARNING: {metadata_path} uses deprecated filename; rename to plugin.yml",
                        file=sys.stderr,
                    )
                    break
        if not metadata_path:
            errors.append(f"Missing {plugin_dir}/plugin.yml")
            continue
        try:
            data = load_yaml(metadata_path) or {}
            plugins.append(validate_plugin(data, plugin_dir))
        except Exception as exc:
            errors.append(f"{metadata_path}: {exc}")

    if errors:
        print("Failed to load marketplace metadata:", file=sys.stderr)
        for err in errors:
            print(f"- {err}", file=sys.stderr)
        return 1

    plugins = sorted(plugins, key=lambda item: item["id"])
    payload = {"plugins": plugins}
    output_path.write_text(json.dumps(payload, indent=2, sort_keys=False) + "\n")
    print(f"Wrote {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
