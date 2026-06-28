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


# GPU execution-provider libraries expected inside a cuda-tagged bundle.
SHERPA_CUDA_LIBS = [
    "libonnxruntime_providers_cuda.so",
    "libonnxruntime_providers_shared.so",
]

# NEEDED/RUNPATH substrings tolerated only for cuda-tagged bundles. CPU bundles
# stay strict and must not reference any of these.
CUDA_DEP_ALLOWLIST = ("libcudart", "libcublas", "libcudnn", "libcuda", "libnvrtc")


def find_bundle(
    bundles_dir: pathlib.Path, plugin_id: str, version: str, accelerator: str = "cpu"
) -> pathlib.Path | None:
    """Find bundle for specific plugin version. Returns None if not found."""
    if accelerator == "cpu":
        bundle_name = f"{plugin_id}-{version}-bundle.tar.zst"
    else:
        bundle_name = f"{plugin_id}-{version}-{accelerator}-bundle.tar.zst"
    bundle_path = bundles_dir / bundle_name
    return bundle_path if bundle_path.exists() else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugins", required=True, help="Path to plugin metadata JSON")
    parser.add_argument("--bundles", required=True, help="Directory with bundle archives")
    parser.add_argument(
        "--accelerator",
        default="cpu",
        help="Accelerator variant to verify ('cpu' default, or e.g. 'cuda').",
    )
    args = parser.parse_args()
    accelerator = args.accelerator.strip().lower()

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
        plugin_version = plugin.get("version")
        if not plugin_version:
            errors.append(f"{plugin_id}: missing version field in metadata")
            continue

        # For variant passes only verify plugins that declare the accelerator.
        if accelerator != "cpu" and accelerator not in plugin.get("accelerators", ["cpu"]):
            continue

        entrypoint = plugin["entrypoint"]
        bundle_path = find_bundle(bundles_dir, plugin_id, plugin_version, accelerator)
        if bundle_path is None:
            # Bundle not found - assume it was published earlier (append-only mode)
            print(f"Skipping {plugin_id} v{plugin_version} ({accelerator} bundle not in {bundles_dir}, likely already published)")
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

            # CPU bundles must stay free of CUDA dependencies; cuda bundles may
            # legitimately link them.
            if accelerator == "cpu":
                cuda_needed = [
                    lib for lib in needed if any(tag in lib for tag in CUDA_DEP_ALLOWLIST)
                ]
                if cuda_needed:
                    errors.append(
                        f"{plugin_id}: CPU bundle links CUDA libraries {cuda_needed}"
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
                if accelerator == "cuda":
                    for cuda_lib in SHERPA_CUDA_LIBS:
                        if not (tmp_path / cuda_lib).exists():
                            errors.append(
                                f"{plugin_id}: cuda bundle missing GPU provider lib {cuda_lib}"
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
