#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

"""Download and convert Helsinki-NLP OPUS-MT models for Candle."""

import sys
from pathlib import Path


def ensure_tokenizers(output_path: Path) -> None:
    """Ensure tokenizer JSON files exist and are compatible with the Rust plugin.

    The Rust plugin expects SentencePiece/Unigram-style tokenizers. If a fast tokenizer isn't
    available via Transformers, we generate Unigram tokenizers using:
      - `vocab.json` for stable token-id mapping (must match model weights)
      - `source.spm` / `target.spm` for token scores (used by Unigram segmentation)
    """

    vocab_path = output_path / "vocab.json"
    source_spm = output_path / "source.spm"
    target_spm = output_path / "target.spm"

    if not vocab_path.exists():
        raise FileNotFoundError(f"vocab.json not found in {output_path}")
    if not source_spm.exists():
        raise FileNotFoundError(f"source.spm not found in {output_path}")
    if not target_spm.exists():
        raise FileNotFoundError(f"target.spm not found in {output_path}")

    try:
        _generate_unigram_tokenizer_json(
            vocab_path=vocab_path,
            spm_path=source_spm,
            output_path=output_path / "source_tokenizer.json",
            missing_token_score=-1.0e9,
        )
        _generate_unigram_tokenizer_json(
            vocab_path=vocab_path,
            spm_path=target_spm,
            output_path=output_path / "target_tokenizer.json",
            missing_token_score=-1.0e9,
        )

        # Back-compat: keep a shared `tokenizer.json` around for older tooling.
        # (Plugin prefers source_tokenizer.json/target_tokenizer.json if present.)
        (output_path / "tokenizer.json").write_text(
            (output_path / "source_tokenizer.json").read_text()
        )
        print("  ✓ Tokenizers generated (source_tokenizer.json, target_tokenizer.json)")
    except Exception as e:
        print(f"  ⚠ Tokenizer generation failed: {e}")
        print("  Falling back to basic tokenizer.json from vocab.json (quality will be poor).")
        create_simple_tokenizer_json(output_path)


def _generate_unigram_tokenizer_json(
    *, vocab_path: Path, spm_path: Path, output_path: Path, missing_token_score: float
) -> None:
    import json

    import sentencepiece as spm
    from tokenizers import Tokenizer
    from tokenizers.decoders import Metaspace as MetaspaceDecoder
    from tokenizers.models import Unigram
    from tokenizers.normalizers import NFKC
    from tokenizers.pre_tokenizers import Metaspace

    with vocab_path.open() as f:
        vocab: dict[str, int] = json.load(f)

    max_id = max(int(i) for i in vocab.values())
    vocab_size = len(vocab)
    if vocab_size != max_id + 1:
        raise ValueError(
            f"vocab.json ids are not contiguous: vocab_size={vocab_size}, max_id={max_id}"
        )

    if "<unk>" not in vocab:
        raise KeyError("vocab.json missing '<unk>' token")
    unk_id = int(vocab["<unk>"])

    sp = spm.SentencePieceProcessor(model_file=str(spm_path))
    sp_scores: dict[str, float] = {
        sp.id_to_piece(i): float(sp.get_score(i)) for i in range(sp.get_piece_size())
    }

    vocab_list: list[tuple[str, float] | None] = [None] * vocab_size
    for token, token_id in vocab.items():
        idx = int(token_id)
        score = sp_scores.get(token, missing_token_score)
        vocab_list[idx] = (token, score)

    if any(x is None for x in vocab_list):
        raise ValueError("vocab.json contains gaps; cannot build Unigram vocab list")

    model = Unigram(vocab=vocab_list, unk_id=unk_id)
    tokenizer = Tokenizer(model)
    tokenizer.normalizer = NFKC()
    # Tokenizers >=0.22 uses `prepend_scheme` instead of `add_prefix_space`.
    tokenizer.pre_tokenizer = Metaspace(
        replacement="▁", prepend_scheme="always", split=True
    )
    tokenizer.decoder = MetaspaceDecoder(
        replacement="▁", prepend_scheme="always", split=True
    )
    tokenizer.save(str(output_path))


def download_and_convert(model_id: str, output_dir: str) -> None:
    """Download model from HuggingFace and convert to Candle format."""
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    # If the model is already present locally, avoid re-downloading and just ensure we have a
    # correct tokenizer configuration. This keeps the script usable offline.
    existing_model = output_path / "model.safetensors"
    existing_vocab = output_path / "vocab.json"
    existing_source_spm = output_path / "source.spm"
    existing_target_spm = output_path / "target.spm"
    if (
        existing_model.exists()
        and existing_vocab.exists()
        and existing_source_spm.exists()
        and existing_target_spm.exists()
    ):
        print(f"  Found existing model files in {output_dir}")
        ensure_tokenizers(output_path)
        return

    # Only require Transformers/Torch when we actually need to download/convert weights.
    from transformers import MarianMTModel, MarianTokenizer

    print(f"  Loading model from HuggingFace ({model_id})...")
    model = MarianMTModel.from_pretrained(model_id)
    tok = MarianTokenizer.from_pretrained(model_id)

    print(f"  Saving tokenizer files to {output_dir}...")
    tok.save_pretrained(str(output_path))

    print(f"  Generating tokenizer JSON files...")
    ensure_tokenizers(output_path)

    print(f"  Saving model as safetensors...")
    model.save_pretrained(str(output_path), safe_serialization=True)

    print(f"  Done! Model saved to {output_dir}")


def create_simple_tokenizer_json(output_path: Path) -> None:
    """Create a basic tokenizer.json from vocab.json for the tokenizers crate."""
    import json

    vocab_path = output_path / "vocab.json"
    if not vocab_path.exists():
        raise FileNotFoundError(f"vocab.json not found in {output_path}")

    with open(vocab_path) as f:
        vocab = json.load(f)

    def get_id(token: str) -> int:
        if token not in vocab:
            raise KeyError(f"Missing required token '{token}' in vocab.json")
        return int(vocab[token])

    pad_id = get_id("<pad>")
    eos_id = get_id("</s>")
    unk_id = get_id("<unk>")

    # Create a minimal tokenizer.json structure
    tokenizer_json = {
        "version": "1.0",
        "truncation": None,
        "padding": None,
        "added_tokens": [
            {"id": pad_id, "content": "<pad>", "single_word": False, "lstrip": False, "rstrip": False, "normalized": False, "special": True},
            {"id": eos_id, "content": "</s>", "single_word": False, "lstrip": False, "rstrip": False, "normalized": False, "special": True},
            {"id": unk_id, "content": "<unk>", "single_word": False, "lstrip": False, "rstrip": False, "normalized": False, "special": True},
        ],
        "normalizer": {"type": "NFC"},
        "pre_tokenizer": {"type": "WhitespaceSplit"},
        "post_processor": None,
        "decoder": None,
        "model": {
            "type": "WordLevel",
            "vocab": vocab,
            "unk_token": "<unk>"
        }
    }

    tokenizer_path = output_path / "tokenizer.json"
    with open(tokenizer_path, "w") as f:
        json.dump(tokenizer_json, f, indent=2)

    print(f"  ✓ Created basic tokenizer.json")


def main() -> int:
    """Main entry point."""
    print("Downloading Helsinki-NLP OPUS-MT models...")
    print()

    try:
        print("Downloading EN->ES model...")
        download_and_convert("Helsinki-NLP/opus-mt-en-es", "models/opus-mt-en-es")
        print()

        print("Downloading ES->EN model...")
        download_and_convert("Helsinki-NLP/opus-mt-es-en", "models/opus-mt-es-en")
        print()

        print("✓ All models downloaded successfully!")
        print("  License: Apache 2.0 (commercial use allowed)")
        return 0

    except ImportError as e:
        print(f"Error: Missing dependency - {e}")
        print()
        print("Please install required packages:")
        print("  pip3 install --user transformers sentencepiece safetensors torch tokenizers")
        return 1

    except Exception as e:
        print(f"Error: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
