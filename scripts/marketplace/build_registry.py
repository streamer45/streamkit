#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

import argparse
import datetime
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
    plugin: dict, version: str, bundles_out: pathlib.Path, work_root: pathlib.Path
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugins", required=True, help="Path to plugin metadata JSON")
    parser.add_argument("--version", required=True, help="Release version (e.g., 1.2.3)")
    parser.add_argument(
        "--bundle-base-url", required=True, help="Base URL for bundle downloads"
    )
    parser.add_argument(
        "--registry-base-url", required=True, help="Base URL for registry metadata"
    )
    parser.add_argument("--bundles-out", required=True, help="Output directory for bundles")
    parser.add_argument("--registry-out", required=True, help="Output directory for registry JSON")
    parser.add_argument("--signing-key", required=True, help="Path to minisign secret key")
    args = parser.parse_args()

    if not args.version.strip():
        print("Release version must be non-empty", file=sys.stderr)
        return 1

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

    metadata = json.loads(plugins_path.read_text())
    plugins = metadata.get("plugins", [])
    if not plugins:
        print("No plugins found in metadata", file=sys.stderr)
        return 1

    bundle_base_url = normalize_base_url(args.bundle_base_url)
    registry_base_url = normalize_base_url(args.registry_base_url)
    published_at = datetime.date.today().isoformat()

    registry_plugins = []
    work_root = registry_out / ".work"
    if work_root.exists():
        shutil.rmtree(work_root)

    for plugin in plugins:
        bundle_info = build_bundle(plugin, args.version, bundles_out, work_root)
        manifest = {
            "schema_version": 1,
            "id": plugin["id"],
            "name": plugin.get("name"),
            "version": args.version,
            "node_kind": plugin["node_kind"],
            "kind": plugin["kind"],
            "description": plugin.get("description"),
            "license": plugin.get("license"),
            "license_url": plugin.get("license_url"),
            "homepage": plugin.get("homepage"),
            "repository": plugin.get("repository"),
            "entrypoint": plugin["entrypoint"],
            "bundle": {
                "url": f"{bundle_base_url}/{bundle_info['bundle_name']}",
                "sha256": bundle_info["sha256"],
                "size_bytes": bundle_info["size_bytes"],
            },
            "compatibility": plugin.get("compatibility"),
            "models": plugin.get("models", []),
        }
        manifest = strip_none(manifest)

        manifest_dir = registry_out / "plugins" / plugin["id"] / args.version
        manifest_path = manifest_dir / "manifest.json"
        write_json(manifest_path, manifest)
        signature_path = sign_manifest(manifest_path, signing_key)

        registry_plugins.append(
            {
                "id": plugin["id"],
                "name": plugin.get("name"),
                "description": plugin.get("description"),
                "latest": args.version,
                "versions": [
                    {
                        "version": args.version,
                        "manifest_url": f"{registry_base_url}/plugins/{plugin['id']}/{args.version}/manifest.json",
                        "signature_url": f"{registry_base_url}/plugins/{plugin['id']}/{args.version}/manifest.minisig",
                        "published_at": published_at,
                    }
                ],
            }
        )

        print(
            f"Built bundle for {plugin['id']} -> {bundle_info['bundle_name']} ({bundle_info['sha256']})"
        )

    index = {"schema_version": 1, "plugins": registry_plugins}
    write_json(registry_out / "index.json", index)

    if work_root.exists():
        shutil.rmtree(work_root)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
