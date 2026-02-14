#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""
Lightweight test for append-only registry behavior.
Tests immutability enforcement and reuse of existing versions.
"""

import json
import pathlib
import shutil
import subprocess
import sys
import tempfile


def setup_test_registry(registry_path: pathlib.Path) -> None:
    """Create a minimal existing registry with one plugin version."""
    plugin_id = "test-plugin"
    version = "0.1.0"

    # Create directory structure
    manifest_dir = registry_path / "plugins" / plugin_id / version
    manifest_dir.mkdir(parents=True, exist_ok=True)

    # Create manifest.json
    manifest = {
        "schema_version": 1,
        "id": plugin_id,
        "name": "Test Plugin",
        "version": version,
        "node_kind": "test",
        "kind": "native",
        "description": "Test plugin for append-only registry",
        "license": "MPL-2.0",
        "entrypoint": "libtest.so",
        "bundle": {
            "url": "https://example.com/test-plugin-0.1.0-bundle.tar.zst",
            "sha256": "abcd1234" * 8,
            "size_bytes": 1024,
        },
        "models": [],
    }
    manifest_path = manifest_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=False) + "\n")

    # Create dummy signature
    signature_path = manifest_dir / "manifest.minisig"
    signature_path.write_text("dummy signature\n")

    # Create index.json
    index = {
        "schema_version": 1,
        "plugins": [
            {
                "id": plugin_id,
                "name": "Test Plugin",
                "description": "Test plugin for append-only registry",
                "latest": version,
                "versions": [
                    {
                        "version": version,
                        "manifest_url": f"https://example.com/registry/plugins/{plugin_id}/{version}/manifest.json",
                        "signature_url": f"https://example.com/registry/plugins/{plugin_id}/{version}/manifest.minisig",
                        "published_at": "2025-01-01",
                    }
                ],
            }
        ],
    }
    index_path = registry_path / "index.json"
    index_path.write_text(json.dumps(index, indent=2, sort_keys=False) + "\n")

    # Create public key
    pubkey_path = registry_path / "streamkit.pub"
    pubkey_path.write_text("dummy public key\n")


def create_test_plugin_metadata(
    plugins_path: pathlib.Path, description: str = "Test plugin for append-only registry"
) -> None:
    """Create plugin metadata JSON."""
    metadata = {
        "plugins": [
            {
                "id": "test-plugin",
                "name": "Test Plugin",
                "version": "0.1.0",
                "node_kind": "test",
                "kind": "native",
                "entrypoint": "libtest.so",
                "artifact": "/tmp/nonexistent.so",  # Won't be used in reuse scenario
                "description": description,
                "license": "MPL-2.0",
                "models": [],
            }
        ]
    }
    plugins_path.write_text(json.dumps(metadata, indent=2) + "\n")


def test_identical_reuse(tmp_dir: pathlib.Path) -> bool:
    """Test that identical plugin metadata reuses existing version without error."""
    print("\n=== Test 1: Identical metadata should reuse existing version ===")

    existing_registry = tmp_dir / "existing_registry"
    setup_test_registry(existing_registry)

    plugins_json = tmp_dir / "plugins.json"
    create_test_plugin_metadata(plugins_json)

    output_registry = tmp_dir / "output_registry"
    bundles_out = tmp_dir / "bundles"
    dummy_key = tmp_dir / "dummy.key"
    dummy_key.write_text("dummy signing key\n")

    # Point to the test's public key, not the repo's
    public_key = existing_registry / "streamkit.pub"

    # Run build_registry in skip-signing mode (we'll mock this by using a dummy key)
    # Since we're reusing, it won't need to sign
    result = subprocess.run(
        [
            "python3",
            "scripts/marketplace/build_registry.py",
            "--plugins",
            str(plugins_json),
            "--existing-registry",
            str(existing_registry),
            "--bundle-base-url",
            "https://example.com/bundles",
            "--registry-base-url",
            "https://example.com/registry",
            "--bundles-out",
            str(bundles_out),
            "--registry-out",
            str(output_registry),
            "--signing-key",
            str(dummy_key),
            "--public-key",
            str(public_key),
        ],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        print(f"FAIL: Expected success but got exit code {result.returncode}")
        print(f"STDOUT: {result.stdout}")
        print(f"STDERR: {result.stderr}")
        return False

    # Check that no bundles were created
    if bundles_out.exists() and list(bundles_out.iterdir()):
        print(f"FAIL: Expected no new bundles but found: {list(bundles_out.iterdir())}")
        return False

    # Check that manifest was copied forward
    manifest_path = output_registry / "plugins" / "test-plugin" / "0.1.0" / "manifest.json"
    if not manifest_path.exists():
        print(f"FAIL: Manifest not found at {manifest_path}")
        return False

    # Check that streamkit.pub was copied
    pubkey_path = output_registry / "streamkit.pub"
    if not pubkey_path.exists():
        print(f"FAIL: Public key not found at {pubkey_path}")
        return False

    print("PASS: Identical metadata reused existing version correctly")
    return True


def test_changed_metadata_fails(tmp_dir: pathlib.Path) -> bool:
    """Test that changed metadata without version bump fails with immutability error."""
    print("\n=== Test 2: Changed metadata without version bump should fail ===")

    existing_registry = tmp_dir / "existing_registry"
    setup_test_registry(existing_registry)

    plugins_json = tmp_dir / "plugins.json"
    # Change description without bumping version
    create_test_plugin_metadata(plugins_json, description="CHANGED DESCRIPTION")

    output_registry = tmp_dir / "output_registry"
    bundles_out = tmp_dir / "bundles"
    dummy_key = tmp_dir / "dummy.key"
    dummy_key.write_text("dummy signing key\n")

    # Point to the test's public key, not the repo's
    public_key = existing_registry / "streamkit.pub"

    result = subprocess.run(
        [
            "python3",
            "scripts/marketplace/build_registry.py",
            "--plugins",
            str(plugins_json),
            "--existing-registry",
            str(existing_registry),
            "--bundle-base-url",
            "https://example.com/bundles",
            "--registry-base-url",
            "https://example.com/registry",
            "--bundles-out",
            str(bundles_out),
            "--registry-out",
            str(output_registry),
            "--signing-key",
            str(dummy_key),
            "--public-key",
            str(public_key),
        ],
        capture_output=True,
        text=True,
    )

    if result.returncode == 0:
        print("FAIL: Expected failure but build succeeded")
        print(f"STDOUT: {result.stdout}")
        return False

    if "already exists in registry but manifest content would change" not in result.stderr:
        print("FAIL: Expected immutability error but got different error")
        print(f"STDERR: {result.stderr}")
        return False

    print("PASS: Immutability check correctly rejected changed metadata")
    return True


def main() -> int:
    """Run all tests."""
    print("Running append-only registry tests...")

    with tempfile.TemporaryDirectory() as tmp_dir_str:
        tmp_dir = pathlib.Path(tmp_dir_str)

        test1_dir = tmp_dir / "test1"
        test1_dir.mkdir()
        test1_passed = test_identical_reuse(test1_dir)

        test2_dir = tmp_dir / "test2"
        test2_dir.mkdir()
        test2_passed = test_changed_metadata_fails(test2_dir)

        print("\n" + "=" * 60)
        if test1_passed and test2_passed:
            print("✓ All tests passed!")
            return 0
        else:
            print("✗ Some tests failed")
            if not test1_passed:
                print("  - Test 1 (identical reuse) FAILED")
            if not test2_passed:
                print("  - Test 2 (immutability check) FAILED")
            return 1


if __name__ == "__main__":
    raise SystemExit(main())
