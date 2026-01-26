<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Marketplace Portability Review

Collected from local artifacts in `plugins/native/*/target/release/*.so` on 2026-01-26.
ldd checks were run on the local dev environment (not a clean container).

| Plugin | Non-glibc NEEDED deps | RUNPATH/RPATH | ldd (local) | Recommendation |
| --- | --- | --- | --- | --- |
| `helsinki` | `libgcc_s.so.1` | — | ok | system dependency (accepted) |
| `kokoro` | `libsherpa-onnx-c-api.so`, `libgcc_s.so.1` | `RUNPATH=/usr/local/lib` | ok | must bundle + `$ORIGIN` |
| `matcha` | `libsherpa-onnx-c-api.so`, `libgcc_s.so.1` | `RUNPATH=/usr/local/lib` | ok | must bundle + `$ORIGIN` |
| `nllb` | `libstdc++.so.6`, `libgcc_s.so.1` | — | ok | system dependency (accepted) |
| `piper` | `libsherpa-onnx-c-api.so`, `libgcc_s.so.1` | `RUNPATH=/usr/local/lib` | ok | must bundle + `$ORIGIN` |
| `sensevoice` | `libsherpa-onnx-c-api.so`, `libstdc++.so.6`, `libgcc_s.so.1` | `RUNPATH=/usr/local/lib` | ok | must bundle + `$ORIGIN` |
| `vad` | `libsherpa-onnx-c-api.so`, `libgcc_s.so.1` | `RUNPATH=/usr/local/lib` | ok | must bundle + `$ORIGIN` |
| `whisper` | `libstdc++.so.6`, `libgcc_s.so.1` | — | ok | system dependency (accepted) |

## Proposed v1 stance (decision checkpoint)

Decision: option 3 (mix).

- Bundle sherpa-onnx shared libs with official bundles and set `RUNPATH=$ORIGIN` (or equivalent).
- Rely on system OpenSSL (libssl/libcrypto) and GCC runtime (libstdc++/libgcc_s).
- When present, `pocket-tts` may rely on system OpenSSL 3.
