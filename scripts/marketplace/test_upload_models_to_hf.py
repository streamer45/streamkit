#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""Unit tests for the marketplace model uploader's archive handling."""

import importlib.util
import pathlib
import tarfile

import pytest

SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent


def _boom(*_args, **_kwargs):
    raise RuntimeError("disk full mid-write")


_spec = importlib.util.spec_from_file_location(
    "upload_models_to_hf", SCRIPTS_DIR / "upload_models_to_hf.py"
)
upload_models_to_hf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(upload_models_to_hf)


class TestArchiveModeCaseInsensitive:
    """Regression for #613: parity with the installer's case-insensitive match."""

    @pytest.mark.parametrize(
        ("name", "expected"),
        [
            ("model.tar.gz", ("model", "w:gz")),
            ("model.TAR.GZ", ("model", "w:gz")),
            ("model.Tar.Zst", ("model", "w:zst")),
            ("MODEL.TGZ", ("MODEL", "w:gz")),
            ("model.zip", None),
        ],
    )
    def test_archive_mode_matches_ignoring_case(self, name, expected):
        assert upload_models_to_hf.archive_mode(name) == expected


class TestMaybeCreateArchiveAtomic:
    """Regression for #612: never reuse a partially-written archive."""

    def _populate(self, models_dir: pathlib.Path) -> None:
        source = models_dir / "model"
        source.mkdir()
        (source / "weights.bin").write_bytes(b"payload")

    def test_creates_archive_and_leaves_no_temp(self, tmp_path):
        self._populate(tmp_path)
        result = upload_models_to_hf.maybe_create_archive(
            tmp_path, "model.tar.gz", create_archives=True
        )
        assert result == tmp_path / "model.tar.gz"
        assert result.exists()
        assert not (tmp_path / "model.tar.gz.tmp").exists()
        with tarfile.open(result, "r:gz") as tar:
            assert "model/weights.bin" in tar.getnames()

    def test_mixed_case_name_creates_archive(self, tmp_path):
        self._populate(tmp_path)
        result = upload_models_to_hf.maybe_create_archive(
            tmp_path, "model.TAR.GZ", create_archives=True
        )
        assert result == tmp_path / "model.TAR.GZ"
        assert result.exists()

    def test_creates_zstd_archive_and_leaves_no_temp(self, tmp_path):
        zstandard = pytest.importorskip("zstandard")
        self._populate(tmp_path)
        result = upload_models_to_hf.maybe_create_archive(
            tmp_path, "model.tar.zst", create_archives=True
        )
        assert result == tmp_path / "model.tar.zst"
        assert result.exists()
        assert not (tmp_path / "model.tar.zst.tmp").exists()
        with result.open("rb") as raw, zstandard.ZstdDecompressor().stream_reader(
            raw
        ) as stream, tarfile.open(fileobj=stream, mode="r|") as tar:
            assert "model/weights.bin" in tar.getnames()

    def test_zstd_partial_write_failure_leaves_no_archive(self, tmp_path, monkeypatch):
        pytest.importorskip("zstandard")
        self._populate(tmp_path)

        real_open = tarfile.open

        def exploding_open(*args, **kwargs):
            tar = real_open(*args, **kwargs)
            monkeypatch.setattr(tar, "add", _boom)
            return tar

        monkeypatch.setattr(upload_models_to_hf.tarfile, "open", exploding_open)

        with pytest.raises(RuntimeError, match="disk full mid-write"):
            upload_models_to_hf.maybe_create_archive(
                tmp_path, "model.tar.zst", create_archives=True
            )

        assert not (tmp_path / "model.tar.zst").exists()
        assert not (tmp_path / "model.tar.zst.tmp").exists()

    def test_partial_write_failure_leaves_no_archive(self, tmp_path, monkeypatch):
        self._populate(tmp_path)

        real_open = tarfile.open

        def exploding_open(*args, **kwargs):
            tar = real_open(*args, **kwargs)
            monkeypatch.setattr(tar, "add", _boom)
            return tar

        monkeypatch.setattr(upload_models_to_hf.tarfile, "open", exploding_open)

        with pytest.raises(RuntimeError, match="disk full mid-write"):
            upload_models_to_hf.maybe_create_archive(
                tmp_path, "model.tar.gz", create_archives=True
            )

        assert not (tmp_path / "model.tar.gz").exists()
        assert not (tmp_path / "model.tar.gz.tmp").exists()
