<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Adding an Official Plugin

When making a plugin official and downloadable from the registry, update all of
the following:

- Plugin source under `plugins/native/<id>/` (crate metadata + README).
- Plugin metadata in `plugins/native/<id>/plugin.yml` (id, version, entrypoint,
  artifact path, models, licenses, homepage/repo).
- Generate `marketplace/official-plugins.json` with
  `scripts/marketplace/generate_official_plugins.py` and commit the result.
- Build list in `scripts/marketplace/build_official_plugins.sh`.
- Build prerequisites in `.github/workflows/release.yml` if new system deps are
  required to compile or package the plugin.
- Bundle/registry smoke check: run `scripts/marketplace/build_registry.py` and
  `scripts/marketplace/verify_bundles.py` locally.
- Portability review: run `scripts/marketplace/verify_bundles.py` which checks
  NEEDED deps, RUNPATH/RPATH, and reports portability issues.
- Docs: add/update the plugin page under
  `docs/src/content/docs/reference/plugins/` and list it in
  `docs/src/content/docs/reference/plugins/index.md` if applicable.
- Runtime shared libs: if the plugin needs bundled `.so` files, ensure the
  bundle includes them and the entrypoint RUNPATH uses `$ORIGIN`, and update the
  portability gate in `scripts/marketplace/verify_bundles.py` as needed.
- **Models**: if the plugin relies on ML models, upload them to the StreamKit
  Hugging Face repo so they remain accessible indefinitely (license permitting).
- **Human review required** before bundling any new third-party shared libraries
  (licensing, security, size, and distro compatibility).
