#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile


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


def extract_bundle(bundle_path: pathlib.Path, dest: pathlib.Path) -> None:
    subprocess.run(
        ["tar", "--zstd", "-xf", str(bundle_path), "-C", str(dest)],
        check=True,
    )


def find_bundle(bundles_dir: pathlib.Path, plugin_id: str) -> pathlib.Path | None:
    suffix = "-bundle.tar.zst"
    matches = [
        path
        for path in bundles_dir.glob("*.tar.zst")
        if path.name.startswith(f"{plugin_id}-") and path.name.endswith(suffix)
    ]
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        raise RuntimeError(f"Multiple bundles found for {plugin_id}: {matches}")
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugins", required=True, help="Path to plugin metadata JSON")
    parser.add_argument("--bundles", required=True, help="Directory with bundle archives")
    args = parser.parse_args()

    plugins_path = pathlib.Path(args.plugins)
    bundles_dir = pathlib.Path(args.bundles)

    if not plugins_path.exists():
        print(f"Missing metadata file: {plugins_path}", file=sys.stderr)
        return 1
    if not bundles_dir.exists():
        print(f"Missing bundles directory: {bundles_dir}", file=sys.stderr)
        return 1

    metadata = json.loads(plugins_path.read_text())
    plugins = metadata.get("plugins", [])
    if not plugins:
        print("No plugins found in metadata", file=sys.stderr)
        return 1

    errors = []

    for plugin in plugins:
        plugin_id = plugin["id"]
        entrypoint = plugin["entrypoint"]
        bundle_path = find_bundle(bundles_dir, plugin_id)
        if bundle_path is None:
            errors.append(f"Missing bundle for {plugin_id} in {bundles_dir}")
            continue

        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = pathlib.Path(tmp_dir)
            extract_bundle(bundle_path, tmp_path)
            entrypoint_path = tmp_path / entrypoint
            if not entrypoint_path.exists():
                errors.append(
                    f"{plugin_id}: missing entrypoint {entrypoint} in {bundle_path.name}"
                )
                continue

            needed, rpaths = readelf_dynamic(entrypoint_path)
            if any("/usr/local/lib" in value for value in rpaths):
                errors.append(
                    f"{plugin_id}: entrypoint has RPATH/RUNPATH referencing /usr/local/lib"
                )

            if "libsherpa-onnx-c-api.so" in needed:
                sherpa_lib = tmp_path / "libsherpa-onnx-c-api.so"
                onnx_lib = tmp_path / "libonnxruntime.so"
                if not sherpa_lib.exists():
                    errors.append(
                        f"{plugin_id}: missing libsherpa-onnx-c-api.so in bundle"
                    )
                if not onnx_lib.exists():
                    errors.append(
                        f"{plugin_id}: missing libonnxruntime.so in bundle"
                    )

    if errors:
        print("Portability verification failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    print("Portability verification passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
