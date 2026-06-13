#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

import argparse
import hashlib
import json
import os
import pathlib
import sys
import tarfile


def sha256_file(path: pathlib.Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def load_metadata(path: pathlib.Path) -> dict:
    return json.loads(path.read_text())


def is_hidden_path(path: pathlib.Path) -> bool:
    return any(part.startswith(".") for part in path.parts)


def archive_mode(file_path: str) -> tuple[str, str] | None:
    if file_path.endswith(".tar.bz2"):
        return file_path[: -len(".tar.bz2")], "w:bz2"
    if file_path.endswith(".tbz2"):
        return file_path[: -len(".tbz2")], "w:bz2"
    if file_path.endswith(".tar.xz"):
        return file_path[: -len(".tar.xz")], "w:xz"
    if file_path.endswith(".txz"):
        return file_path[: -len(".txz")], "w:xz"
    if file_path.endswith(".tar.gz"):
        return file_path[: -len(".tar.gz")], "w:gz"
    if file_path.endswith(".tgz"):
        return file_path[: -len(".tgz")], "w:gz"
    if file_path.endswith(".tar"):
        return file_path[: -len(".tar")], "w"
    return None


def maybe_create_archive(
    models_dir: pathlib.Path, file_path: str, create_archives: bool
) -> pathlib.Path | None:
    if not create_archives:
        return None
    archive_path = models_dir / file_path
    if archive_path.exists():
        return archive_path
    if pathlib.Path(file_path).parent != pathlib.Path("."):
        return None
    mode = archive_mode(file_path)
    if mode is None:
        return None
    base_name, tar_mode = mode
    source_dir = models_dir / base_name
    if not source_dir.is_dir():
        return None

    def filter_hidden(tar_info: tarfile.TarInfo) -> tarfile.TarInfo | None:
        if is_hidden_path(pathlib.Path(tar_info.name)):
            return None
        return tar_info

    if tar_mode == "w:xz":
        try:
            import lzma  # noqa: F401
        except ImportError:
            sys.exit("Python was built without lzma support; cannot create .tar.xz archives")

    print(f"Creating archive {archive_path} from {source_dir}...")
    with tarfile.open(archive_path, tar_mode) as tar:
        tar.add(source_dir, arcname=base_name, filter=filter_hidden)
    return archive_path


def find_local_path(
    models_dir: pathlib.Path, file_path: str, create_archives: bool
) -> pathlib.Path | None:
    candidate = models_dir / file_path
    if candidate.exists():
        return candidate
    candidate = maybe_create_archive(models_dir, file_path, create_archives)
    if candidate is not None and candidate.exists():
        return candidate
    basename = pathlib.Path(file_path).name
    candidate = models_dir / basename
    if candidate.exists():
        return candidate
    return None


def collect_models(metadata: dict, repo_id: str) -> list[tuple[str, str | None]]:
    files: list[tuple[str, str | None]] = []
    for plugin in metadata.get("plugins", []):
        for model in plugin.get("models", []):
            if model.get("source") != "huggingface":
                continue
            if model.get("repo_id") != repo_id:
                continue
            for file_path in model.get("files", []):
                files.append((file_path, model.get("sha256")))
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--metadata",
        default="marketplace/official-plugins.json",
        help="Path to official plugins metadata JSON",
    )
    parser.add_argument(
        "--models-dir",
        default="models",
        help="Local directory containing model files",
    )
    parser.add_argument(
        "--repo",
        default="streamkit/whisper-models",
        help="Hugging Face repo to upload into (e.g. streamkit/whisper-models)",
    )
    parser.add_argument(
        "--revision",
        default="main",
        help="Target branch or revision in the Hugging Face repo",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print planned uploads without pushing files",
    )
    parser.add_argument(
        "--verify-hashes",
        action="store_true",
        help="Fail if local hashes do not match manifest hashes",
    )
    parser.add_argument(
        "--create-archives",
        action="store_true",
        help="Create .tar.bz2/.tar.gz archives from model directories when missing",
    )
    args = parser.parse_args()

    metadata_path = pathlib.Path(args.metadata)
    models_dir = pathlib.Path(args.models_dir)
    if not metadata_path.exists():
        print(f"Missing metadata file: {metadata_path}", file=sys.stderr)
        return 1
    if not models_dir.exists():
        print(f"Missing models directory: {models_dir}", file=sys.stderr)
        return 1

    metadata = load_metadata(metadata_path)
    files = collect_models(metadata, args.repo)
    if not files:
        print(f"No Hugging Face models found for repo '{args.repo}'")
        return 0

    uploads: list[tuple[pathlib.Path, str]] = []
    for file_path, expected_hash in files:
        local_path = find_local_path(models_dir, file_path, args.create_archives)
        if local_path is None:
            print(f"Missing local model file for '{file_path}'", file=sys.stderr)
            return 1
        actual_hash = sha256_file(local_path)
        if expected_hash and expected_hash != actual_hash:
            message = (
                f"Hash mismatch for {local_path} ({actual_hash} != {expected_hash})"
            )
            if args.verify_hashes:
                print(message, file=sys.stderr)
                return 1
            print(f"Warning: {message}", file=sys.stderr)
        uploads.append((local_path, file_path))

    for local_path, repo_path in uploads:
        print(f"Upload: {local_path} -> {args.repo}:{repo_path}")

    if args.dry_run:
        return 0

    token = os.environ.get("HF_TOKEN")
    if not token:
        print("HF_TOKEN is required for uploading to Hugging Face", file=sys.stderr)
        return 1

    try:
        from huggingface_hub import HfApi
    except ImportError:
        print("Missing dependency: pip install huggingface_hub", file=sys.stderr)
        return 1

    api = HfApi(token=token)
    api.create_repo(repo_id=args.repo, repo_type="model", exist_ok=True)

    for local_path, repo_path in uploads:
        api.upload_file(
            path_or_fileobj=str(local_path),
            path_in_repo=repo_path,
            repo_id=args.repo,
            repo_type="model",
            revision=args.revision,
            token=token,
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
