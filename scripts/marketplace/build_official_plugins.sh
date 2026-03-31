#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

python3 scripts/marketplace/generate_official_plugins.py

plugins=$(python3 - <<'PY'
import json
import pathlib
import sys

plugins_path = pathlib.Path("marketplace/official-plugins.json")
metadata = json.loads(plugins_path.read_text())
plugin_ids = [plugin["id"] for plugin in metadata.get("plugins", [])]

native_root = pathlib.Path("plugins/native")
native_dirs = [path.name for path in native_root.iterdir() if path.is_dir()]

missing = sorted(set(native_dirs) - set(plugin_ids))
if missing:
    print(
        "Missing entries in marketplace/official-plugins.json for: "
        + ", ".join(missing),
        file=sys.stderr,
    )
    sys.exit(1)

print("\n".join(plugin_ids))
PY
)

while IFS= read -r plugin; do
  if [ -z "${plugin}" ]; then
    continue
  fi
  if [ ! -d "plugins/native/${plugin}" ]; then
    echo "Missing plugin directory: plugins/native/${plugin}" >&2
    exit 1
  fi
  echo "Building native plugin: ${plugin}"
  (
    cd "plugins/native/${plugin}"
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(cd ../../.. && pwd)/target/plugins}" cargo build --release
  )
done <<< "${plugins}"
