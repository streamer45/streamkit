#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

import shutil
import sys
from pathlib import Path

try:
    from huggingface_hub import hf_hub_download
except ImportError:  # pragma: no cover - runtime dependency check
    print("Missing dependency: huggingface-hub", file=sys.stderr)
    print("Install with: pip3 install --user huggingface-hub", file=sys.stderr)
    sys.exit(1)

WEIGHTS_REV = "427e3d61b276ed69fdd03de0d185fa8a8d97fc5b"
TOKENIZER_REV = "d4fdd22ae8c8e1cb3634e150ebeff1dab2d16df3"

PREDEFINED_VOICES = [
    "alba",
    "marius",
    "javert",
    "jean",
    "fantine",
    "cosette",
    "eponine",
    "azelma",
]


def download(repo_id: str, filename: str, revision: str | None = None) -> Path:
    path = hf_hub_download(
        repo_id=repo_id,
        filename=filename,
        revision=revision,
    )
    path = Path(path)
    print(f"Downloaded {repo_id}/{filename} -> {path}")
    return path


def copy_to_output(src: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dest)
    print(f"Copied {src} -> {dest}")


def main() -> None:
    print("Downloading Pocket TTS model weights and tokenizer...")
    print("Note: kyutai/pocket-tts is gated; set HF_TOKEN to authenticate.")

    output_dir = Path("models/pocket-tts")
    embeddings_dir = output_dir / "embeddings"

    weights_path = download(
        "kyutai/pocket-tts",
        "tts_b6369a24.safetensors",
        WEIGHTS_REV,
    )
    copy_to_output(weights_path, output_dir / "tts_b6369a24.safetensors")

    tokenizer_path = download(
        "kyutai/pocket-tts-without-voice-cloning",
        "tokenizer.model",
        TOKENIZER_REV,
    )
    copy_to_output(tokenizer_path, output_dir / "tokenizer.model")

    print("Downloading Pocket TTS voice embeddings...")
    for voice in PREDEFINED_VOICES:
        voice_path = download(
            "kyutai/pocket-tts-without-voice-cloning",
            f"embeddings/{voice}.safetensors",
        )
        copy_to_output(voice_path, embeddings_dir / f"{voice}.safetensors")

    print("Pocket TTS downloads complete.")
    print(f"Local model directory: {output_dir}")


if __name__ == "__main__":
    main()
