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


SAMPLE_BUNDLE = {
    "url": "https://example.com/test-plugin-1.0.0-bundle.tar.zst",
    "sha256": "ab" * 32,
    "size_bytes": 1024,
}

CUDA_VARIANT = {
    "accelerator": "cuda",
    "url": "https://example.com/test-plugin-1.0.0-cuda-bundle.tar.zst",
    "sha256": "cd" * 32,
    "size_bytes": 2048,
}


class TestBuildManifestVariants:
    """Variant handling in build_registry / check_registry manifest builders."""

    def test_no_variants_omits_field(self):
        manifest = build_registry.build_manifest(SAMPLE_PLUGIN, "1.0.0", SAMPLE_BUNDLE)
        assert "variants" not in manifest

    def test_empty_variants_omits_field(self):
        manifest = build_registry.build_manifest(
            SAMPLE_PLUGIN, "1.0.0", SAMPLE_BUNDLE, variants=[]
        )
        assert "variants" not in manifest

    def test_variants_inserted_after_bundle(self):
        manifest = build_registry.build_manifest(
            SAMPLE_PLUGIN, "1.0.0", SAMPLE_BUNDLE, variants=[CUDA_VARIANT]
        )
        keys = list(manifest.keys())
        assert manifest["variants"] == [CUDA_VARIANT]
        assert keys.index("variants") == keys.index("bundle") + 1

    def test_check_registry_carries_variants(self):
        manifest = check_registry_versions.build_manifest_from_plugin(
            SAMPLE_PLUGIN, SAMPLE_BUNDLE, variants=[CUDA_VARIANT]
        )
        assert manifest["variants"] == [CUDA_VARIANT]

    def test_build_and_check_manifests_match_with_variants(self):
        built = build_registry.build_manifest(
            SAMPLE_PLUGIN, "1.0.0", SAMPLE_BUNDLE, variants=[CUDA_VARIANT]
        )
        checked = check_registry_versions.build_manifest_from_plugin(
            {**SAMPLE_PLUGIN, "version": "1.0.0"}, SAMPLE_BUNDLE, variants=[CUDA_VARIANT]
        )
        assert built == checked


SAMPLE_ASSET = {
    "type_id": "slint",
    "label": "Slint Files",
    "extensions": ["slint"],
    "max_size_bytes": 1048576,
    "content_type": "text",
    "icon_hint": "code",
    "node_param": "slint_file",
    "system_dir": "samples/slint/system",
}


class TestBuildManifestAssets:
    """Asset-type declarations must survive into manifest.json (and the embedded
    plugin.yml) so the server can rediscover them after install/extraction."""

    def test_assets_included_when_present(self):
        plugin = {**SAMPLE_PLUGIN, "assets": [SAMPLE_ASSET]}
        manifest = build_registry.build_manifest(plugin, "1.0.0", bundle_block=None)
        assert manifest["assets"] == [SAMPLE_ASSET]

    def test_assets_omitted_when_absent(self):
        manifest = build_registry.build_manifest(SAMPLE_PLUGIN, "1.0.0", bundle_block=None)
        assert "assets" not in manifest

    def test_assets_omitted_when_empty(self):
        plugin = {**SAMPLE_PLUGIN, "assets": []}
        manifest = build_registry.build_manifest(plugin, "1.0.0", bundle_block=None)
        assert "assets" not in manifest

    def test_build_and_check_manifests_match_with_assets(self):
        plugin = {**SAMPLE_PLUGIN, "assets": [SAMPLE_ASSET]}
        built = build_registry.build_manifest(plugin, "1.0.0", SAMPLE_BUNDLE)
        checked = check_registry_versions.build_manifest_from_plugin(
            {**plugin, "version": "1.0.0"}, SAMPLE_BUNDLE
        )
        assert built == checked


class TestWritePluginYml:
    """build_bundle embeds a plugin.yml so raw-extraction consumers (the demo)
    recover asset types the server only reads from plugin.yml."""

    def test_writes_parseable_yaml_with_assets(self, tmp_path):
        import yaml

        plugin = {**SAMPLE_PLUGIN, "assets": [SAMPLE_ASSET]}
        manifest = build_registry.build_manifest(plugin, "1.0.0", bundle_block=None)
        build_registry.write_plugin_yml(tmp_path, manifest)

        yml_path = tmp_path / "plugin.yml"
        assert yml_path.exists()
        parsed = yaml.safe_load(yml_path.read_text())
        assert parsed["assets"] == [SAMPLE_ASSET]
        assert parsed["id"] == "test-plugin"


class TestEnsureSherpaRuntime:
    """ensure_sherpa_runtime copies extra GPU libs for cuda variants."""

    def _make_libs(self, lib_dir: pathlib.Path, names) -> None:
        lib_dir.mkdir(parents=True, exist_ok=True)
        for name in names:
            (lib_dir / name).write_bytes(b"\x7fELF")

    def test_cpu_copies_core_libs_only(self, tmp_path, monkeypatch):
        lib_dir = tmp_path / "libs"
        self._make_libs(
            lib_dir,
            build_registry.SHERPA_CORE_LIBS + build_registry.SHERPA_CUDA_LIBS,
        )
        monkeypatch.setenv("SHERPA_ONNX_LIB_DIR", str(lib_dir))
        work = tmp_path / "work"
        work.mkdir()
        build_registry.ensure_sherpa_runtime(work, "cpu")
        for name in build_registry.SHERPA_CORE_LIBS:
            assert (work / name).exists()
        for name in build_registry.SHERPA_CUDA_LIBS:
            assert not (work / name).exists()

    def test_cuda_copies_gpu_provider_libs(self, tmp_path, monkeypatch):
        lib_dir = tmp_path / "libs"
        self._make_libs(
            lib_dir,
            build_registry.SHERPA_CORE_LIBS + build_registry.SHERPA_CUDA_LIBS,
        )
        monkeypatch.setenv("SHERPA_ONNX_LIB_DIR", str(lib_dir))
        work = tmp_path / "work"
        work.mkdir()
        build_registry.ensure_sherpa_runtime(work, "cuda")
        for name in build_registry.SHERPA_CORE_LIBS + build_registry.SHERPA_CUDA_LIBS:
            assert (work / name).exists()


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
