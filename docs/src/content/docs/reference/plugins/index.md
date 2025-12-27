---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Plugin Reference
description: Official plugin nodes and their parameters
---

This section documents the **official plugins** shipped in this repository.

You can also discover whatever is currently loaded in your running server:

```bash
curl http://localhost:4545/api/v1/plugins
curl http://localhost:4545/api/v1/schema/nodes | jq '.[] | select(.kind | startswith("plugin::"))'
```

> [!NOTE]
> The second command requires `jq`.

## Official plugins (8)

- [`plugin::native::helsinki`](./plugin-native-helsinki/) (original kind: `helsinki`)
- [`plugin::native::kokoro`](./plugin-native-kokoro/) (original kind: `kokoro`)
- [`plugin::native::matcha`](./plugin-native-matcha/) (original kind: `matcha`)
- [`plugin::native::nllb`](./plugin-native-nllb/) (original kind: `nllb`)
- [`plugin::native::piper`](./plugin-native-piper/) (original kind: `piper`)
- [`plugin::native::sensevoice`](./plugin-native-sensevoice/) (original kind: `sensevoice`)
- [`plugin::native::vad`](./plugin-native-vad/) (original kind: `vad`)
- [`plugin::native::whisper`](./plugin-native-whisper/) (original kind: `whisper`)
