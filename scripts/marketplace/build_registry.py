#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

import argparse
import datetime
import difflib
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys


def sha256_file(path: pathlib.Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def readelf_dynamic(path: pathlib.Path) -> tuple[list[str], list[str]]:
    result = subprocess.run(
        ["readelf", "-d", str(path)],
        check=True,
        text=True,
        capture_output=True,
    )
    needed = []
    rpaths = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if "(NEEDED)" in line and "Shared library:" in line:
            lib = line.split("[", 1)[-1].split("]", 1)[0]
            needed.append(lib)
        elif "(RUNPATH)" in line or "(RPATH)" in line:
            value = line.split("[", 1)[-1].split("]", 1)[0]
            rpaths.append(value)
    return needed, rpaths


def normalize_base_url(url: str) -> str:
    return url.rstrip("/")


def ensure_dir(path: pathlib.Path) -> None:
    path.mkdir(parents=True, exist_ok=True)


def copy_file(src: pathlib.Path, dest: pathlib.Path) -> None:
    ensure_dir(dest.parent)
    shutil.copy2(src, dest)


def require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise RuntimeError(f"Missing required tool: {name}")


def ensure_sherpa_runtime(work_dir: pathlib.Path) -> None:
    lib_dir = pathlib.Path(os.environ.get("SHERPA_ONNX_LIB_DIR", "/usr/local/lib"))
    sherpa_libs = ["libsherpa-onnx-c-api.so", "libonnxruntime.so"]
    for lib in sherpa_libs:
        src = lib_dir / lib
        if not src.exists():
            raise FileNotFoundError(f"Missing sherpa runtime library: {src}")
        copy_file(src, work_dir / lib)


def set_runpath_origin(target: pathlib.Path) -> None:
    require_tool("patchelf")
    subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", str(target)], check=True)


def build_bundle(
    plugin: dict,
    version: str,
    bundles_out: pathlib.Path,
    work_root: pathlib.Path,
    embedded_manifest: dict | None = None,
) -> dict:
    plugin_id = plugin["id"]
    artifact = pathlib.Path(plugin["artifact"])
    entrypoint = pathlib.Path(plugin["entrypoint"])

    if not artifact.exists():
        raise FileNotFoundError(f"Missing artifact: {artifact}")

    work_dir = work_root / f"{plugin_id}-{version}"
    if work_dir.exists():
        shutil.rmtree(work_dir)
    ensure_dir(work_dir)

    entrypoint_path = work_dir / entrypoint
    copy_file(artifact, entrypoint_path)

    needed, _ = readelf_dynamic(entrypoint_path)
    if "libsherpa-onnx-c-api.so" in needed:
        ensure_sherpa_runtime(work_dir)
        set_runpath_origin(entrypoint_path)

    for extra in plugin.get("extra_files", []):
        if isinstance(extra, str):
            src = pathlib.Path(extra)
            dest = pathlib.Path(src.name)
        else:
            src = pathlib.Path(extra["source"])
            dest = pathlib.Path(extra.get("dest", src.name))
        copy_file(src, work_dir / dest)

    if embedded_manifest is not None:
        write_json(work_dir / "manifest.json", embedded_manifest)

    bundle_name = f"{plugin_id}-{version}-bundle.tar.zst"
    bundle_path = bundles_out / bundle_name
    ensure_dir(bundles_out)

    subprocess.run(
        [
            "tar",
            "--zstd",
            "-cf",
            str(bundle_path),
            "-C",
            str(work_dir),
            ".",
        ],
        check=True,
    )

    return {
        "bundle_name": bundle_name,
        "bundle_path": bundle_path,
        "sha256": sha256_file(bundle_path),
        "size_bytes": bundle_path.stat().st_size,
    }


def write_json(path: pathlib.Path, payload: dict) -> None:
    ensure_dir(path.parent)
    path.write_text(json.dumps(payload, indent=2, sort_keys=False))


def strip_none(payload: dict) -> dict:
    return {key: value for key, value in payload.items() if value is not None}


def dump_manifest_bytes(manifest: dict) -> bytes:
    """Produce canonical manifest bytes matching write_json formatting."""
    return (json.dumps(manifest, indent=2, sort_keys=False) + "\n").encode("utf-8")


def build_manifest(
    plugin: dict,
    plugin_version: str,
    bundle_block: dict | None,
) -> dict:
    """Build manifest dict from plugin metadata and bundle info."""
    manifest = {
        "schema_version": 1,
        "id": plugin["id"],
        "name": plugin.get("name"),
        "version": plugin_version,
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
    }
    return strip_none(manifest)


def sign_manifest(manifest_path: pathlib.Path, signing_key: pathlib.Path) -> pathlib.Path:
    signature_path = manifest_path.with_name("manifest.minisig")
    subprocess.run(
        [
            "minisign",
            "-S",
            "-s",
            str(signing_key),
            "-m",
            str(manifest_path),
            "-x",
            str(signature_path),
        ],
        check=True,
    )
    return signature_path


def is_prerelease(version: str) -> bool:
    """Check if version has prerelease identifier (before any +build)."""
    # Strip build metadata first
    if "+" in version:
        version = version.split("+", 1)[0]
    return "-" in version


def parse_semver_key(version: str) -> tuple:
    """
    Parse SemVer into a sortable key tuple.
    Returns (major, minor, patch, is_stable, prerelease_parts).

    Per SemVer 2.0.0:
    - Build metadata (+...) is ignored for precedence
    - Prerelease versions have lower precedence than normal versions
    - Prerelease identifiers are compared by:
      * Numeric identifiers are compared as integers
      * Alphanumeric identifiers are compared lexically
      * Numeric identifiers have lower precedence than non-numeric
    """
    # Strip build metadata (everything after +)
    if "+" in version:
        version = version.split("+", 1)[0]

    # Split into base version and prerelease
    if "-" in version:
        base, prerelease = version.split("-", 1)
        is_stable = False
    else:
        base, prerelease = version, ""
        is_stable = True

    # Parse base version
    parts = base.split(".")
    if len(parts) != 3:
        raise ValueError(f"Invalid semver base: {version}")
    try:
        major, minor, patch = map(int, parts)
    except ValueError as exc:
        raise ValueError(f"Invalid semver numbers in: {version}") from exc

    # Parse prerelease identifiers
    prerelease_parts = []
    if prerelease:
        for part in prerelease.split("."):
            # Try to parse as int, otherwise keep as string
            try:
                # Numeric identifier
                prerelease_parts.append((0, int(part)))
            except ValueError:
                # Alphanumeric identifier
                prerelease_parts.append((1, part))

    # Return sortable key:
    # - (major, minor, patch) compares numerically
    # - is_stable=True sorts higher than is_stable=False for same base version
    # - prerelease_parts compares element-wise per SemVer rules
    return (major, minor, patch, is_stable, prerelease_parts)


def load_existing_registry(registry_path: pathlib.Path) -> tuple[dict[tuple[str, str], dict], dict]:
    """
    Load existing registry and return:
    - Map of (plugin_id, version) -> {manifest, signature_path}
    - Map of plugin_id -> {versions: [...], metadata}
    """
    existing = {}
    plugins_dir = registry_path / "plugins"
    if not plugins_dir.exists():
        return existing, {}

    # Load manifests and signatures
    for plugin_dir in plugins_dir.iterdir():
        if not plugin_dir.is_dir():
            continue
        plugin_id = plugin_dir.name

        for version_dir in plugin_dir.iterdir():
            if not version_dir.is_dir():
                continue
            version = version_dir.name

            manifest_path = version_dir / "manifest.json"
            signature_path = version_dir / "manifest.minisig"

            if manifest_path.exists() and signature_path.exists():
                manifest = json.loads(manifest_path.read_text())
                existing[(plugin_id, version)] = {
                    "manifest": manifest,
                    "manifest_path": manifest_path,
                    "signature_path": signature_path,
                }

    # Load index.json for published_at timestamps and metadata
    index_map = {}
    index_path = registry_path / "index.json"
    if index_path.exists():
        index_data = json.loads(index_path.read_text())
        for plugin in index_data.get("plugins", []):
            index_map[plugin["id"]] = plugin

    return existing, index_map


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugins", required=True, help="Path to plugin metadata JSON")
    parser.add_argument(
        "--bundle-url-template",
        required=True,
        help="URL template for bundle downloads with {plugin_id} and {version} placeholders",
    )
    parser.add_argument(
        "--registry-base-url", required=True, help="Base URL for registry metadata"
    )
    parser.add_argument("--bundles-out", required=True, help="Output directory for bundles")
    parser.add_argument("--registry-out", required=True, help="Output directory for registry JSON")
    parser.add_argument("--signing-key", required=True, help="Path to minisign secret key")
    parser.add_argument(
        "--existing-registry",
        help="Path to existing registry directory (for append-only mode)",
    )
    parser.add_argument(
        "--public-key",
        help="Path to minisign public key to include in registry (default: docs/public/registry/streamkit.pub if exists)",
    )
    parser.add_argument(
        "--new-plugins-out",
        help="Path to write JSON file listing newly built plugins (id + version)",
    )
    args = parser.parse_args()

    plugins_path = pathlib.Path(args.plugins)
    bundles_out = pathlib.Path(args.bundles_out)
    registry_out = pathlib.Path(args.registry_out)
    signing_key = pathlib.Path(args.signing_key)

    if not plugins_path.exists():
        print(f"Missing metadata file: {plugins_path}", file=sys.stderr)
        return 1
    if not signing_key.exists():
        print(f"Missing minisign key: {signing_key}", file=sys.stderr)
        return 1

    # Load existing registry if provided
    existing_registry = {}
    existing_index_map = {}
    if args.existing_registry:
        existing_registry_path = pathlib.Path(args.existing_registry)
        if existing_registry_path.exists():
            existing_registry, existing_index_map = load_existing_registry(existing_registry_path)
            print(f"Loaded {len(existing_registry)} existing plugin versions from registry")

    metadata = json.loads(plugins_path.read_text())
    plugins = metadata.get("plugins", [])
    if not plugins:
        print("No plugins found in metadata", file=sys.stderr)
        return 1

    bundle_url_template = args.bundle_url_template.rstrip("/")
    registry_base_url = normalize_base_url(args.registry_base_url)
    published_at = datetime.date.today().isoformat()
    new_plugins = []

    # Track all versions per plugin for index.json
    plugin_versions_map = {}  # plugin_id -> list of version entries

    work_root = registry_out / ".work"
    if work_root.exists():
        shutil.rmtree(work_root)

    for plugin in plugins:
        plugin_id = plugin["id"]
        plugin_version = plugin.get("version")
        if not plugin_version:
            print(f"ERROR: Plugin {plugin_id} missing version field", file=sys.stderr)
            return 1

        key = (plugin_id, plugin_version)

        # Check if this version already exists in the registry
        if key in existing_registry:
            # Verify immutability: check if republishing with same version would change manifest
            existing = existing_registry[key]
            existing_manifest = existing["manifest"]

            # Build would-be manifest using current plugin fields but existing bundle
            would_be_manifest = build_manifest(
                plugin,
                plugin_version,
                existing_manifest["bundle"],
            )

            # Compare parsed JSON objects (robust to formatting differences like trailing newlines)
            if existing_manifest != would_be_manifest:
                print(
                    f"ERROR: {plugin_id}@{plugin_version} already exists in registry "
                    f"but manifest content would change; bump plugin.yml version.",
                    file=sys.stderr,
                )
                print(f"Existing manifest: {existing['manifest_path']}", file=sys.stderr)
                # Show diff for debugging
                existing_json = json.dumps(existing_manifest, indent=2, sort_keys=False)
                would_be_json = json.dumps(would_be_manifest, indent=2, sort_keys=False)
                diff = difflib.unified_diff(
                    existing_json.splitlines(keepends=True),
                    would_be_json.splitlines(keepends=True),
                    fromfile="existing",
                    tofile="would-be",
                )
                print("Manifest differences:", file=sys.stderr)
                print("".join(diff), file=sys.stderr)
                return 1

            print(f"Reusing existing {plugin_id} v{plugin_version}")

            # Copy forward existing manifest and signature
            manifest_dir = registry_out / "plugins" / plugin_id / plugin_version
            manifest_path = manifest_dir / "manifest.json"
            signature_path = manifest_dir / "manifest.minisig"

            ensure_dir(manifest_dir)
            shutil.copy2(existing["manifest_path"], manifest_path)
            shutil.copy2(existing["signature_path"], signature_path)
        else:
            # Build new version
            # Build the embedded manifest first (without bundle block) so it
            # gets included inside the archive for offline inspection.
            embedded_manifest = build_manifest(plugin, plugin_version, bundle_block=None)
            bundle_info = build_bundle(
                plugin, plugin_version, bundles_out, work_root,
                embedded_manifest=embedded_manifest,
            )
            bundle_base = bundle_url_template.format(
                plugin_id=plugin_id, version=plugin_version,
            )
            bundle_block = {
                "url": f"{bundle_base}/{bundle_info['bundle_name']}",
                "sha256": bundle_info["sha256"],
                "size_bytes": bundle_info["size_bytes"],
            }
            manifest = build_manifest(plugin, plugin_version, bundle_block)

            manifest_dir = registry_out / "plugins" / plugin_id / plugin_version
            manifest_path = manifest_dir / "manifest.json"
            write_json(manifest_path, manifest)
            sign_manifest(manifest_path, signing_key)

            new_plugins.append({"id": plugin_id, "version": plugin_version})

            print(
                f"Built {plugin_id} v{plugin_version} -> {bundle_info['bundle_name']} ({bundle_info['sha256']})"
            )

    # Build index.json by merging all versions (existing + new)
    # First, collect all versions from existing registry
    for (plugin_id, version), existing in existing_registry.items():
        if plugin_id not in plugin_versions_map:
            plugin_versions_map[plugin_id] = []

        # Get published_at from existing index.json if available
        existing_published_at = published_at
        if plugin_id in existing_index_map:
            for ver_entry in existing_index_map[plugin_id].get("versions", []):
                if ver_entry.get("version") == version:
                    existing_published_at = ver_entry.get("published_at", published_at)
                    break

        plugin_versions_map[plugin_id].append(
            {
                "version": version,
                "manifest_url": f"{registry_base_url}/plugins/{plugin_id}/{version}/manifest.json",
                "signature_url": f"{registry_base_url}/plugins/{plugin_id}/{version}/manifest.minisig",
                "published_at": existing_published_at,
            }
        )

    # Add current plugins (may be new or update existing entries)
    for plugin in plugins:
        plugin_id = plugin["id"]
        plugin_version = plugin["version"]

        if plugin_id not in plugin_versions_map:
            plugin_versions_map[plugin_id] = []

        # Check if this version is already in the list
        already_exists = any(v["version"] == plugin_version for v in plugin_versions_map[plugin_id])
        if not already_exists:
            plugin_versions_map[plugin_id].append(
                {
                    "version": plugin_version,
                    "manifest_url": f"{registry_base_url}/plugins/{plugin_id}/{plugin_version}/manifest.json",
                    "signature_url": f"{registry_base_url}/plugins/{plugin_id}/{plugin_version}/manifest.minisig",
                    "published_at": published_at,
                }
            )

    # Build final index with sorted versions and computed latest
    # Include all plugins that have versions in plugin_versions_map
    plugin_metadata = {p["id"]: p for p in plugins}
    registry_plugins = []

    for plugin_id in sorted(plugin_versions_map.keys()):
        versions = plugin_versions_map[plugin_id]

        # Sort versions by semver (highest precedence first)
        versions.sort(key=lambda v: parse_semver_key(v["version"]), reverse=True)

        # Determine latest: prefer max stable version, otherwise max prerelease
        stable_versions = [v for v in versions if not is_prerelease(v["version"])]
        if stable_versions:
            latest = stable_versions[0]["version"]
        elif versions:
            latest = versions[0]["version"]
        else:
            # Fallback (shouldn't happen)
            latest = versions[0]["version"] if versions else "0.0.0"

        # Get plugin metadata from current plugins or existing registry
        plugin_meta = plugin_metadata.get(plugin_id, {})
        if not plugin_meta and existing_registry:
            # Try to get metadata from first existing version
            for (pid, ver), existing in existing_registry.items():
                if pid == plugin_id:
                    plugin_meta = existing["manifest"]
                    break

        registry_plugins.append(
            {
                "id": plugin_id,
                "name": plugin_meta.get("name", plugin_id),
                "description": plugin_meta.get("description"),
                "latest": latest,
                "versions": versions,
            }
        )

    index = {"schema_version": 1, "plugins": registry_plugins}
    write_json(registry_out / "index.json", index)

    # Copy public key if available
    public_key_path = None
    if args.public_key:
        public_key_path = pathlib.Path(args.public_key)
    else:
        # Try default location
        default_key = pathlib.Path("docs/public/registry/streamkit.pub")
        if default_key.exists():
            public_key_path = default_key

    if public_key_path and public_key_path.exists():
        dest_key = registry_out / "streamkit.pub"
        shutil.copy2(public_key_path, dest_key)
        print(f"Copied public key to registry: {dest_key}")
    elif args.public_key:
        print(f"WARNING: Specified public key not found: {args.public_key}", file=sys.stderr)

    if args.new_plugins_out:
        new_plugins_path = pathlib.Path(args.new_plugins_out)
        write_json(new_plugins_path, {"plugins": new_plugins})
        print(f"Wrote {len(new_plugins)} new plugin(s) to {new_plugins_path}")

    if work_root.exists():
        shutil.rmtree(work_root)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
