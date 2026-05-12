#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""Unit tests for build_registry and check_registry_versions manifest builders."""

import importlib.util
import pathlib
import sys

import pytest

SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent

# Import build_registry module by path (it has no package structure).
_br_spec = importlib.util.spec_from_file_location(
    "build_registry", SCRIPTS_DIR / "build_registry.py"
)
build_registry = importlib.util.module_from_spec(_br_spec)
_br_spec.loader.exec_module(build_registry)

# Import check_registry_versions module by path.
_cr_spec = importlib.util.spec_from_file_location(
    "check_registry_versions", SCRIPTS_DIR / "check_registry_versions.py"
)
check_registry_versions = importlib.util.module_from_spec(_cr_spec)
_cr_spec.loader.exec_module(check_registry_versions)


SAMPLE_PLUGIN = {
    "id": "test-plugin",
    "name": "Test Plugin",
    "node_kind": "test_node",
    "kind": "native",
    "entrypoint": "libtest.so",
    "description": "A test plugin",
    "license": "MPL-2.0",
    "repo": "https://github.com/streamer45/streamkit",
}


class TestBuildManifestRepoField:
    """Regression tests for #307: plugin.yml uses ``repo`` not ``repository``."""

    def test_build_manifest_maps_repo_to_repository(self):
        manifest = build_registry.build_manifest(SAMPLE_PLUGIN, "1.0.0", bundle_block=None)
        assert manifest["repository"] == "https://github.com/streamer45/streamkit"

    def test_check_registry_maps_repo_to_repository(self):
        manifest = check_registry_versions.build_manifest_from_plugin(
            SAMPLE_PLUGIN, bundle_block=None
        )
        assert manifest["repository"] == "https://github.com/streamer45/streamkit"

    def test_build_manifest_omits_repository_when_repo_absent(self):
        plugin = {k: v for k, v in SAMPLE_PLUGIN.items() if k != "repo"}
        manifest = build_registry.build_manifest(plugin, "1.0.0", bundle_block=None)
        assert "repository" not in manifest

    def test_check_registry_omits_repository_when_repo_absent(self):
        plugin = {k: v for k, v in SAMPLE_PLUGIN.items() if k != "repo"}
        manifest = check_registry_versions.build_manifest_from_plugin(plugin, bundle_block=None)
        assert "repository" not in manifest


class TestVerifyExistingSignature:
    """Tests for verify_existing_signature used during manifest reuse."""

    def test_returns_true_when_public_key_is_none(self, tmp_path):
        manifest = tmp_path / "manifest.json"
        manifest.write_text("{}")
        sig = tmp_path / "manifest.minisig"
        sig.write_text("dummy")
        assert build_registry.verify_existing_signature(manifest, sig, None) is True

    def test_returns_true_when_public_key_missing(self, tmp_path):
        manifest = tmp_path / "manifest.json"
        manifest.write_text("{}")
        sig = tmp_path / "manifest.minisig"
        sig.write_text("dummy")
        missing = tmp_path / "nonexistent.pub"
        assert build_registry.verify_existing_signature(manifest, sig, missing) is True
