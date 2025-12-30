// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// REUSE-IgnoreStart
// This file contains template strings with SPDX headers for generated markdown files.
// These are not license declarations for this source file.

// This is a doc generation tool - the format_push_string and uninlined_format_args
// patterns are idiomatic here for building markdown content incrementally.
#![allow(clippy::format_push_string, clippy::uninlined_format_args)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use schemars::schema_for;
use serde_json::Value;
use streamkit_core::{InputPin, NodeDefinition, NodeRegistry, OutputPin, PinCardinality};
use streamkit_server::config::Config;

fn main() -> Result<()> {
    let repo_root = std::env::current_dir().context("failed to read current directory")?;
    let docs_reference_dir = repo_root.join("docs/src/content/docs/reference");

    let args: Vec<String> = std::env::args().collect();
    let out_dir = match args.as_slice() {
        [_] => docs_reference_dir,
        [_, flag, dir] if flag == "--out" => repo_root.join(dir),
        _ => {
            return Err(anyhow!(
				"usage: gen-docs-reference [--out <path>]\n(defaults to docs/src/content/docs/reference)"
			));
        },
    };

    let nodes_dir = out_dir.join("nodes");
    let plugins_dir = out_dir.join("plugins");
    let packets_dir = out_dir.join("packets");

    fs::create_dir_all(&nodes_dir).context("failed to create nodes output dir")?;
    fs::create_dir_all(&plugins_dir).context("failed to create plugins output dir")?;
    fs::create_dir_all(&packets_dir).context("failed to create packets output dir")?;

    // --- Built-in nodes (core runtime nodes) ---
    let mut registry = NodeRegistry::new();
    streamkit_nodes::register_nodes(&mut registry, None, std::collections::HashMap::default());

    let mut built_in_nodes = registry.definitions();
    add_synthetic_oneshot_nodes(&mut built_in_nodes);
    built_in_nodes.retain(|def| !def.kind.starts_with("plugin::"));
    built_in_nodes.sort_by(|a, b| a.kind.cmp(&b.kind));

    clean_generated_pages(&nodes_dir)?;
    for def in &built_in_nodes {
        let slug = slugify_kind(&def.kind);
        let page_path = nodes_dir.join(format!("{slug}.md"));
        let markdown = render_node_page(def)?;
        write_if_changed(&page_path, &markdown)?;
    }

    // Index page for built-in nodes.
    let nodes_index = render_nodes_index(&built_in_nodes);
    write_if_changed(&nodes_dir.join("index.md"), &nodes_index)?;

    // --- Official native plugins (from repo artifacts) ---
    let mut official_plugins = load_official_native_plugins(&repo_root)?;
    official_plugins.sort_by(|a, b| a.kind.cmp(&b.kind));

    clean_generated_pages(&plugins_dir)?;
    for def in &official_plugins {
        let slug = slugify_kind(&def.kind);
        let page_path = plugins_dir.join(format!("{slug}.md"));
        let markdown = render_plugin_page(def)?;
        write_if_changed(&page_path, &markdown)?;
    }

    let plugins_index = render_plugins_index(&official_plugins);
    write_if_changed(&plugins_dir.join("index.md"), &plugins_index)?;

    // --- Packet types (core) ---
    generate_packet_docs(&packets_dir)?;

    // --- Configuration reference ---
    generate_config_docs(&out_dir)?;

    Ok(())
}

fn generate_packet_docs(packets_dir: &Path) -> Result<()> {
    clean_generated_pages(packets_dir)?;

    let packet_types = packet_type_entries();

    let index_md = render_packets_index(&packet_types);
    write_if_changed(&packets_dir.join("index.md"), &index_md)?;

    for entry in packet_types {
        let page_path = packets_dir.join(format!("{}.md", entry.slug));
        let markdown = render_packet_page(&entry)?;
        write_if_changed(&page_path, &markdown)?;
    }

    Ok(())
}

fn add_synthetic_oneshot_nodes(defs: &mut Vec<NodeDefinition>) {
    use streamkit_core::types::PacketType;

    defs.push(NodeDefinition {
        kind: "streamkit::http_input".to_string(),
        description: Some(
            "Synthetic input node for oneshot HTTP pipelines. \
             Receives binary data from the HTTP request body."
                .to_string(),
        ),
        param_schema: serde_json::json!({}),
        inputs: vec![],
        outputs: vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }],
        categories: vec!["transport".to_string(), "oneshot".to_string()],
        bidirectional: false,
    });

    defs.push(NodeDefinition {
        kind: "streamkit::http_output".to_string(),
        description: Some(
            "Synthetic output node for oneshot HTTP pipelines. \
             Sends binary data as the HTTP response body."
                .to_string(),
        ),
        param_schema: serde_json::json!({}),
        inputs: vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }],
        outputs: vec![],
        categories: vec!["transport".to_string(), "oneshot".to_string()],
        bidirectional: false,
    });
}

fn clean_generated_pages(dir: &Path) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if path.file_name() == Some(OsStr::new("index.md")) {
            continue;
        }
        if path.extension() == Some(OsStr::new("md")) {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    let existing = fs::read_to_string(path).ok();
    if existing.as_deref() == Some(contents) {
        return Ok(());
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn slugify_kind(kind: &str) -> String {
    let mut out = String::with_capacity(kind.len());
    let mut prev_dash = false;
    for ch in kind.chars() {
        let is_ok = ch.is_ascii_alphanumeric();
        if is_ok {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
            continue;
        }
        if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[derive(Clone, Copy)]
struct PacketTypeEntry {
    id: &'static str,
    slug: &'static str,
    label: &'static str,
    kind_repr: &'static str,
    runtime_repr: &'static str,
}

fn packet_type_entries() -> Vec<PacketTypeEntry> {
    vec![
        PacketTypeEntry {
            id: "RawAudio",
            slug: "raw-audio",
            label: "Raw Audio",
            kind_repr: "PacketType::RawAudio(AudioFormat)",
            runtime_repr: "Packet::Audio(AudioFrame)",
        },
        PacketTypeEntry {
            id: "OpusAudio",
            slug: "opus-audio",
            label: "Opus Audio",
            kind_repr: "PacketType::OpusAudio",
            runtime_repr: "Packet::Binary { data, metadata, .. }",
        },
        PacketTypeEntry {
            id: "Text",
            slug: "text",
            label: "Text",
            kind_repr: "PacketType::Text",
            runtime_repr: "Packet::Text(Arc<str>)",
        },
        PacketTypeEntry {
            id: "Transcription",
            slug: "transcription",
            label: "Transcription",
            kind_repr: "PacketType::Transcription",
            runtime_repr: "Packet::Transcription(Arc<TranscriptionData>)",
        },
        PacketTypeEntry {
            id: "Custom",
            slug: "custom",
            label: "Custom",
            kind_repr: "PacketType::Custom { type_id }",
            runtime_repr: "Packet::Custom(Arc<CustomPacketData>)",
        },
        PacketTypeEntry {
            id: "Binary",
            slug: "binary",
            label: "Binary",
            kind_repr: "PacketType::Binary",
            runtime_repr: "Packet::Binary { data, content_type, metadata }",
        },
        PacketTypeEntry {
            id: "Any",
            slug: "any",
            label: "Any",
            kind_repr: "PacketType::Any",
            runtime_repr: "Type-system only (matches any PacketType)",
        },
        PacketTypeEntry {
            id: "Passthrough",
            slug: "passthrough",
            label: "Passthrough",
            kind_repr: "PacketType::Passthrough",
            runtime_repr: "Type inference marker (output type = input type)",
        },
    ]
}

fn render_packets_index(entries: &[PacketTypeEntry]) -> String {
    let registry = streamkit_core::packet_meta::packet_type_registry();

    let mut out = String::new();
    out.push_str(
        r"---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Packet Types
description: Supported packet types and their structures
---

StreamKit pipelines are **type-checked** using `PacketType` and exchange runtime data using `Packet`.

At runtime, the server also exposes UI-oriented packet metadata (labels, colors, compatibility rules):

```bash
curl http://localhost:4545/api/v1/schema/packets
```

## Types

| PacketType | Link | Runtime representation | Notes |
| --- | --- | --- | --- |
",
    );

    for entry in entries {
        let notes = registry
            .iter()
            .find(|m| m.id == entry.id)
            .map_or_else(|| "—".to_string(), packet_meta_notes);
        out.push_str(&format!(
            "| `{}` | [**{}**](./{}/) | `{}` | {} |\n",
            entry.id, entry.label, entry.slug, entry.runtime_repr, notes
        ));
    }

    out.push_str(
        "
## Serialization

`PacketType` serializes as:

- A string for unit variants (e.g., `\"Text\"`, `\"Binary\"`).
- An object for payload variants (e.g., `{\"RawAudio\": {\"sample_rate\": 48000, ...}}`).\n",
    );

    out
}

fn packet_meta_notes(meta: &streamkit_core::packet_meta::PacketTypeMeta) -> String {
    let compat = match &meta.compatibility {
        streamkit_core::packet_meta::Compatibility::Any => "compat: any".to_string(),
        streamkit_core::packet_meta::Compatibility::Exact => "compat: exact".to_string(),
        streamkit_core::packet_meta::Compatibility::StructFieldWildcard { fields } => {
            let fields = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
            format!("compat: wildcard fields ({fields})")
        },
    };

    if meta.color.is_empty() {
        compat
    } else {
        format!("{compat}, color: `{}`", meta.color)
    }
}

fn render_packet_page(entry: &PacketTypeEntry) -> Result<String> {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("# SPDX-FileCopyrightText: © 2025 StreamKit Contributors\n");
    out.push_str("# SPDX-License-Identifier: MPL-2.0\n");
    out.push_str(&format!("title: {}\n", yaml_string(entry.label)));
    out.push_str(&format!(
        "description: {}\n",
        yaml_string(&format!("PacketType {} structure", entry.id))
    ));
    out.push_str("---\n\n");

    out.push_str(&format!("`PacketType` id: `{}`\n\n", entry.id));
    out.push_str(&format!("Type system: `{}`\n\n", entry.kind_repr));
    out.push_str(&format!("Runtime: `{}`\n\n", entry.runtime_repr));

    if entry.id == "Custom" {
        out.push_str(
            r#"## Why `Custom` exists
`Custom` is StreamKit's **extensibility escape hatch**: it lets plugins and pipelines exchange
structured data **without adding new built-in packet variants**.

It was designed to:

- Keep the core packet set small and stable (important for UIs and SDKs).
- Enable fast iteration for plugin-defined events and typed messages.
- Preserve type-checking via `type_id` so pipelines still validate before running.
- Stay user-friendly over JSON APIs (`encoding: "json"` is debuggable and easy to inspect).

## When to use it
Use `Custom` when you need **structured, typed messages** that don't fit existing packet types, for example:

- Plugin-defined events (e.g. VAD, moderation, scene triggers, rich status updates).
- Application-level envelopes (e.g. tool results, routing hints, structured logs).
- Telemetry-like events (see below) that you want to treat as first-class data.

Prefer other packet types when they fit:

- Audio frames/streams: `/reference/packets/raw-audio/` or `/reference/packets/opus-audio/`
- Plain strings: `/reference/packets/text/`
- Opaque bytes, blobs, or media: `/reference/packets/binary/`
- Speech-to-text results: `/reference/packets/transcription/`

## Type IDs, versioning, and compatibility
`type_id` is the **routing key** for `Custom` and is part of the type system.

- Compatibility: `PacketType::Custom { type_id: "a@1" }` only connects to the same `type_id`.
  If you truly want "any custom", use `PacketType::Any` on the input pin.
- Versioning: include a major version suffix like `@1` and bump it for breaking payload changes.
- Namespacing: use a stable, collision-resistant prefix (examples below).

Examples used in this repo:

- `core::telemetry/event@1` (telemetry envelope used on the WebSocket bus)
- `plugin::native::vad/vad-event@1` (VAD-style events)

## Payload conventions
`data` is schema-less JSON: treat it as **untrusted input** and validate it in consumers.

For "event"-shaped payloads, a common convention is an `event_type` string inside `data`:

```json
{
  "type_id": "core::telemetry/event@1",
  "encoding": "json",
  "data": { "event_type": "vad.start", "source": "mic" },
  "metadata": { "timestamp_us": 1735257600000000 }
}
```

Related docs:

- WebSocket telemetry events: `/reference/websocket-api/#telemetry-events-nodetelemetry`
- Nodes that observe/emit telemetry: `/reference/nodes/core-telemetry-tap/`, `/reference/nodes/core-telemetry-out/`

## When a core packet type is a better fit
`Custom` is great for iteration, but adding a new core packet type can be worth it when:

- The payload is **high-volume / performance-sensitive** (zero-copy, binary codecs, large frames).
- The payload needs **canonical semantics** across the ecosystem (multiple nodes, UIs, SDKs).
- There are **well-defined fields** that benefit from first-class schema/compat rules (not just `type_id`).
- The payload should be **universally inspectable/renderable** in the UI (timelines, previews, editors).

In those cases, open a GitHub issue describing the use case and examples (or send a PR). The goal is to keep
the built-in packet set small and stable, and graduate widely useful patterns out of `Custom` when needed.

"#,
        );
    }

    let registry = streamkit_core::packet_meta::packet_type_registry();
    if let Some(meta) = registry.iter().find(|m| m.id == entry.id) {
        out.push_str("## UI Metadata\n");
        out.push_str(&format!("- `label`: `{}`\n", meta.label));
        out.push_str(&format!("- `color`: `{}`\n", meta.color));
        if let Some(tpl) = &meta.display_template {
            out.push_str(&format!("- `display_template`: `{}`\n", tpl));
        }
        out.push_str(&format!("- `{}`\n\n", packet_meta_notes(meta)));
    }

    out.push_str("## Structure\n");
    out.push_str(&render_packet_structure(entry)?);
    Ok(out)
}

fn render_packet_structure(entry: &PacketTypeEntry) -> Result<String> {
    match entry.id {
        "RawAudio" => {
            let mut out = String::new();
            out.push_str(
                r"Raw audio is defined by an `AudioFormat` in the type system and carried as `Packet::Audio(AudioFrame)` at runtime.

### PacketType payload (`AudioFormat`)

",
            );
            let schema = serde_json::to_value(schema_for!(streamkit_core::types::AudioFormat))
                .context("failed to generate AudioFormat schema")?;
            out.push_str(&render_object_fields(&schema, &schema, 0));
            out.push_str(&render_raw_schema(&schema));

            out.push_str(
                r"
### Runtime payload (`AudioFrame`)

`AudioFrame` is optimized for zero-copy fan-out. It contains:

- `sample_rate` (u32)
- `channels` (u16)
- `samples` (interleaved f32 array)
- `metadata` (`PacketMetadata`, optional)
",
            );

            Ok(out)
        },
        "Transcription" => {
            let mut out = String::new();
            out.push_str("Transcriptions are carried as `Packet::Transcription(Arc<TranscriptionData>)`.\n\n");

            let schema = serde_json::to_value(schema_for!(streamkit_core::types::TranscriptionData))
                .context("failed to generate TranscriptionData schema")?;
            out.push_str(&render_object_fields(&schema, &schema, 0));
            out.push_str(&render_raw_schema(&schema));
            Ok(out)
        },
        "Custom" => {
            let mut out = String::new();
            out.push_str(
                "Custom packets are carried as `Packet::Custom(Arc<CustomPacketData>)`.\n\n",
            );

            let schema = serde_json::to_value(schema_for!(streamkit_core::types::CustomPacketData))
                .context("failed to generate CustomPacketData schema")?;
            out.push_str(&render_object_fields(&schema, &schema, 0));
            out.push_str(&render_raw_schema(&schema));
            Ok(out)
        },
        "Binary" => Ok(
            r#"Binary packets are carried as:

```json
{
  "data": "<base64>",
  "content_type": "application/octet-stream",
  "metadata": { "timestamp_us": 0, "duration_us": 20000, "sequence": 42 }
}
```

Notes:

- `data` is base64-encoded for JSON transport.
- `content_type` is optional and may be `null`.
"#
            .to_string(),
        ),
        "OpusAudio" => Ok(
            r"Opus packets use the `OpusAudio` packet type, but the runtime payload is still `Packet::Binary`.

The Opus codec nodes encode/decode using `Packet::Binary { data, metadata, .. }` and label pins as `OpusAudio`.
"
            .to_string(),
        ),
        "Text" => Ok("Text packets are carried as `Packet::Text(Arc<str>)`.\n".to_string()),
        "Any" => Ok(
            r"`Any` is a special type that matches any packet type during validation.

It does not describe a specific runtime structure.
"
            .to_string(),
        ),
        "Passthrough" => Ok(
            r"`Passthrough` is a type inference marker (output type = input type).

In oneshot pipelines it is resolved during compilation. In dynamic pipelines it may be resolved when packets flow.
"
            .to_string(),
        ),
        _ => Ok("No structure available.\n".to_string()),
    }
}

fn render_nodes_index(defs: &[NodeDefinition]) -> String {
    let mut by_namespace: BTreeMap<String, Vec<&NodeDefinition>> = BTreeMap::new();
    for def in defs {
        let ns = def.kind.split("::").next().unwrap_or("unknown").to_string();
        by_namespace.entry(ns).or_default().push(def);
    }

    let mut out = String::new();
    out.push_str(
        r"---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Node Reference
description: Built-in node kinds and parameter reference
---

This section documents the **built-in nodes** shipped with StreamKit, including their pins and parameter schema.

Available nodes (including loaded plugins) are also discoverable at runtime:

```bash
curl http://localhost:4545/api/v1/schema/nodes
```

Notes:

- The response is permission-filtered based on your role.
- Two synthetic nodes exist for oneshot-only HTTP streaming: `streamkit::http_input` and `streamkit::http_output`.

",
    );

    for (ns, items) in by_namespace {
        out.push_str(&format!("\n## `{ns}` ({})\n\n", items.len()));
        for def in items {
            let slug = slugify_kind(&def.kind);
            out.push_str(&format!("- [`{}`](./{}/)\n", def.kind, slug));
        }
    }

    out
}

fn render_plugins_index(defs: &[PluginNodeDoc]) -> String {
    let mut out = String::new();
    out.push_str(
        r#"---
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

"#,
    );

    out.push_str(&format!("## Official plugins ({})\n\n", defs.len()));
    for def in defs {
        let slug = slugify_kind(&def.kind);
        out.push_str(&format!(
            "- [`{}`](./{}/) (original kind: `{}`)\n",
            def.kind, slug, def.original_kind
        ));
    }

    out
}

#[allow(clippy::unnecessary_wraps)] // Consistent API with other render functions
fn render_node_page(def: &NodeDefinition) -> Result<String> {
    let schema = &def.param_schema;
    // Prefer explicit description from NodeDefinition, fall back to schema description
    let description = def
        .description
        .as_deref()
        .or_else(|| schema.get("description").and_then(Value::as_str))
        .unwrap_or("Built-in node reference");

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("# SPDX-FileCopyrightText: © 2025 StreamKit Contributors\n");
    out.push_str("# SPDX-License-Identifier: MPL-2.0\n");
    out.push_str(&format!("title: {}\n", yaml_string(&def.kind)));
    out.push_str(&format!("description: {}\n", yaml_string(description)));
    out.push_str("---\n\n");

    out.push_str(&format!("`kind`: `{}`\n\n", def.kind));

    // Add description as prose if present
    if let Some(desc) = &def.description {
        out.push_str(desc);
        out.push_str("\n\n");
    }

    if !def.categories.is_empty() {
        out.push_str("## Categories\n");
        for cat in &def.categories {
            out.push_str(&format!("- `{}`\n", cat));
        }
        out.push('\n');
    }

    out.push_str("## Pins\n");
    out.push_str(&render_pins(&def.inputs, &def.outputs));

    out.push_str("\n## Parameters\n");
    out.push_str(&render_params(schema));
    out.push('\n');
    out.push_str(&render_raw_schema(schema));

    Ok(out)
}

#[derive(Clone)]
struct PluginNodeDoc {
    kind: String,
    original_kind: String,
    description: Option<String>,
    categories: Vec<String>,
    inputs: Vec<InputPin>,
    outputs: Vec<OutputPin>,
    param_schema: Value,
    source_path: String,
    /// Example pipeline YAML content if available
    example_pipeline: Option<String>,
}

#[allow(clippy::unnecessary_wraps)] // Consistent API with other render functions
fn render_plugin_page(def: &PluginNodeDoc) -> Result<String> {
    let schema = &def.param_schema;
    // Prefer explicit description from plugin metadata, fall back to schema description
    let description = def
        .description
        .as_deref()
        .or_else(|| schema.get("description").and_then(Value::as_str))
        .unwrap_or("Plugin node reference");

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("# SPDX-FileCopyrightText: © 2025 StreamKit Contributors\n");
    out.push_str("# SPDX-License-Identifier: MPL-2.0\n");
    out.push_str(&format!("title: {}\n", yaml_string(&def.kind)));
    out.push_str(&format!("description: {}\n", yaml_string(description)));
    out.push_str("---\n\n");

    out.push_str(&format!("`kind`: `{}` (original kind: `{}`)\n\n", def.kind, def.original_kind));

    // Add description as prose if present
    if let Some(desc) = &def.description {
        out.push_str(desc);
        out.push_str("\n\n");
    }

    out.push_str(&format!("Source: `{}`\n\n", def.source_path));

    if !def.categories.is_empty() {
        out.push_str("## Categories\n");
        for cat in &def.categories {
            out.push_str(&format!("- `{}`\n", cat));
        }
        out.push('\n');
    }

    out.push_str("## Pins\n");
    out.push_str(&render_pins(&def.inputs, &def.outputs));

    out.push_str("\n## Parameters\n");
    out.push_str(&render_params(schema));
    out.push('\n');

    // Add example pipeline if available
    if let Some(example) = &def.example_pipeline {
        out.push_str("## Example Pipeline\n\n");
        out.push_str("```yaml\n");
        out.push_str(example);
        if !example.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    out.push_str(&render_raw_schema(schema));

    Ok(out)
}

fn render_pins(inputs: &[InputPin], outputs: &[OutputPin]) -> String {
    let mut out = String::new();
    out.push_str("### Inputs\n");
    if inputs.is_empty() {
        out.push_str("No inputs.\n");
    } else {
        for pin in inputs {
            let types =
                pin.accepts_types.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", ");
            out.push_str(&format!(
                "- `{}` accepts `{}` ({})\n",
                pin.name,
                types,
                cardinality_label(&pin.cardinality)
            ));
        }
    }
    out.push('\n');

    out.push_str("### Outputs\n");
    if outputs.is_empty() {
        out.push_str("No outputs.\n");
    } else {
        for pin in outputs {
            out.push_str(&format!(
                "- `{}` produces `{:?}` ({})\n",
                pin.name,
                pin.produces_type,
                cardinality_label(&pin.cardinality)
            ));
        }
    }
    out
}

const fn cardinality_label(cardinality: &PinCardinality) -> &'static str {
    match cardinality {
        PinCardinality::One => "one",
        PinCardinality::Broadcast => "broadcast",
        PinCardinality::Dynamic { .. } => "dynamic",
    }
}

fn render_params(schema: &Value) -> String {
    let resolved = resolve_ref(schema, schema).unwrap_or(schema);
    let props = resolved.get("properties").and_then(Value::as_object);
    let required: BTreeSet<String> = resolved
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
        .unwrap_or_default();

    let Some(props) = props else {
        return "No parameters.\n".to_string();
    };
    if props.is_empty() {
        return "No parameters.\n".to_string();
    }

    let mut out = String::new();
    out.push_str("| Name | Type | Required | Default | Description |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");

    // Track nested object schemas we should expand below the table.
    let mut nested: Vec<(String, Value)> = Vec::new();

    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    for key in keys {
        let prop_schema = &props[key];
        let prop_resolved = resolve_ref(schema, prop_schema).unwrap_or(prop_schema);

        let type_label = schema_type_label(schema, prop_resolved);
        let required_label = if required.contains(key.as_str()) { "yes" } else { "no" };
        let default_label = prop_resolved.get("default").map(json_one_line).unwrap_or_default();
        let description = schema_description(prop_resolved);

        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            key,
            type_label,
            required_label,
            if default_label.is_empty() {
                "—".to_string()
            } else {
                format!("`{}`", default_label)
            },
            if description.is_empty() { "—".to_string() } else { description }
        ));

        if let Some(obj_schema) = object_schema_to_expand(schema, prop_resolved) {
            nested.push((key.clone(), obj_schema));
        }
    }

    for (name, obj_schema) in nested {
        out.push_str(&format!("\n### `{}` fields\n\n", name));
        out.push_str(&render_object_fields(schema, &obj_schema, 0));
    }

    out
}

fn render_object_fields(root: &Value, obj_schema: &Value, depth: usize) -> String {
    if depth > 6 {
        return "Depth limit reached.\n".to_string();
    }

    let resolved = resolve_ref(root, obj_schema).unwrap_or(obj_schema);
    let props = resolved.get("properties").and_then(Value::as_object);
    let required: BTreeSet<String> = resolved
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
        .unwrap_or_default();

    let Some(props) = props else {
        return "No structured fields.\n".to_string();
    };
    if props.is_empty() {
        return "No structured fields.\n".to_string();
    }

    let mut out = String::new();
    out.push_str("| Name | Type | Required | Default | Description |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");

    let mut nested: Vec<(String, Value)> = Vec::new();

    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    for key in keys {
        let prop_schema = &props[key];
        let prop_resolved = resolve_ref(root, prop_schema).unwrap_or(prop_schema);

        let type_label = schema_type_label(root, prop_resolved);
        let required_label = if required.contains(key.as_str()) { "yes" } else { "no" };
        let default_label = prop_resolved.get("default").map(json_one_line).unwrap_or_default();
        let description = schema_description(prop_resolved);

        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            key,
            type_label,
            required_label,
            if default_label.is_empty() {
                "—".to_string()
            } else {
                format!("`{}`", default_label)
            },
            if description.is_empty() { "—".to_string() } else { description }
        ));

        if let Some(obj_schema) = object_schema_to_expand(root, prop_resolved) {
            nested.push((key.clone(), obj_schema));
        }
    }

    for (name, schema) in nested {
        out.push_str(&format!("\n#### `{}` fields\n\n", name));
        out.push_str(&render_object_fields(root, &schema, depth + 1));
    }

    out
}

fn object_schema_to_expand(root: &Value, schema: &Value) -> Option<Value> {
    let resolved = resolve_ref(root, schema).unwrap_or(schema);

    if resolved.get("type").and_then(Value::as_str) == Some("object")
        || resolved
            .get("type")
            .and_then(Value::as_array)
            .is_some_and(|arr| arr.iter().any(|t| t.as_str() == Some("object")))
    {
        return Some(resolved.clone());
    }

    if resolved.get("type").and_then(Value::as_str) == Some("array") {
        let items = resolved.get("items")?;
        let items_resolved = resolve_ref(root, items).unwrap_or(items);
        if items_resolved.get("type").and_then(Value::as_str) == Some("object") {
            return Some(items_resolved.clone());
        }
    }

    None
}

fn schema_type_label(root: &Value, schema: &Value) -> String {
    let resolved = resolve_ref(root, schema).unwrap_or(schema);

    if let Some(ty) = resolved.get("type") {
        match ty {
            Value::String(s) => return type_with_format(root, resolved, s),
            Value::Array(arr) => {
                let mut types: Vec<String> =
                    arr.iter().filter_map(Value::as_str).map(ToString::to_string).collect();
                types.sort();
                let label = types.join(" | ");
                return type_with_format(root, resolved, &label);
            },
            _ => {},
        }
    }

    for key in ["oneOf", "anyOf"] {
        if let Some(options) = resolved.get(key).and_then(Value::as_array) {
            let mut labels = Vec::new();
            for opt in options {
                let opt_resolved = resolve_ref(root, opt).unwrap_or(opt);
                if let Some(s) = opt_resolved.get("type").and_then(Value::as_str) {
                    labels.push(type_with_format(root, opt_resolved, s));
                } else {
                    labels.push("value".to_string());
                }
            }
            labels.sort();
            labels.dedup();
            return labels.join(" | ");
        }
    }

    "value".to_string()
}

fn type_with_format(root: &Value, schema: &Value, base: &str) -> String {
    if base == "array" {
        if let Some(items) = schema.get("items") {
            let item_ty = schema_type_label(root, items);
            return format!("array<{}>", item_ty);
        }
    }

    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        return format!("{base} ({format})");
    }

    if let Some(en) = schema.get("enum").and_then(Value::as_array) {
        let vals = en.iter().map(json_one_line).collect::<Vec<_>>().join(", ");
        return format!("{base} enum[{vals}]");
    }

    base.to_string()
}

fn schema_description(schema: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(desc) = schema.get("description").and_then(Value::as_str) {
        let desc = desc.replace('\n', "<br />");
        parts.push(desc);
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
        parts.push(format!("min: `{}`", trim_float(min)));
    }
    if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
        parts.push(format!("max: `{}`", trim_float(max)));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        parts.push(format!("pattern: `{}`", pattern));
    }

    if parts.is_empty() {
        return String::new();
    }
    parts.join("<br />")
}

fn trim_float(v: f64) -> String {
    let s = v.to_string();
    if s.ends_with(".0") {
        s.trim_end_matches(".0").to_string()
    } else {
        s
    }
}

fn render_raw_schema(schema: &Value) -> String {
    let pretty = serde_json::to_string_pretty(schema).unwrap_or_else(|_| "{}".to_string());
    format!(
        r"
<details>
<summary>Raw JSON Schema</summary>

```json
{pretty}
```

</details>
"
    )
}

fn json_one_line(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => serde_json::to_string(v).unwrap_or_else(|_| String::new()),
    }
}

fn yaml_string(s: &str) -> String {
    // Minimal YAML string quoting for frontmatter values.
    let escaped = s.replace('\\', "\\\\").replace('\"', "\\\"");
    format!("\"{escaped}\"")
}

fn resolve_ref<'a>(root: &'a Value, schema: &'a Value) -> Option<&'a Value> {
    let ref_str = schema.get("$ref")?.as_str()?;
    if !ref_str.starts_with("#/") {
        return None;
    }

    let pointer = ref_str.trim_start_matches('#');
    let mut current = root;
    for token in pointer.split('/').skip(1) {
        let token = token.replace("~1", "/").replace("~0", "~");
        current = current.get(&token)?;
    }
    Some(current)
}

fn load_official_native_plugins(repo_root: &Path) -> Result<Vec<PluginNodeDoc>> {
    let mut out = Vec::new();
    let plugin_paths = find_official_native_plugin_artifacts(repo_root)?;

    for path in plugin_paths {
        // Help the dynamic loader find adjacent shared libraries (e.g., ONNX runtime).
        if let Some(parent) = path.parent() {
            prepend_to_library_path(parent);
        }

        let plugin = streamkit_plugin_native::LoadedNativePlugin::load(&path)
            .with_context(|| format!("failed to load native plugin {}", path.display()))?;
        let meta = plugin.metadata().clone();
        let kind = streamkit_plugin_native::namespaced_kind(&meta.kind)
            .with_context(|| format!("invalid plugin kind '{}'", meta.kind))?;

        // Look for example pipeline for this plugin
        let example_pipeline = find_example_pipeline(repo_root, &kind);

        out.push(PluginNodeDoc {
            kind,
            original_kind: meta.kind,
            description: meta.description,
            categories: meta.categories,
            inputs: meta.inputs,
            outputs: meta.outputs,
            param_schema: meta.param_schema,
            source_path: path
                .strip_prefix(repo_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            example_pipeline,
        });
    }

    Ok(out)
}

/// Finds an example pipeline that uses a specific plugin kind.
/// Searches in samples/pipelines/oneshot/ for pipelines containing the plugin.
fn find_example_pipeline(repo_root: &Path, plugin_kind: &str) -> Option<String> {
    let samples_dir = repo_root.join("samples/pipelines/oneshot");
    if !samples_dir.exists() {
        return None;
    }

    // Map plugin kinds to their example pipeline files
    let example_map: std::collections::HashMap<&str, &str> = [
        ("plugin::native::whisper", "speech_to_text.yml"),
        ("plugin::native::kokoro", "kokoro-tts.yml"),
        ("plugin::native::vad", "vad-demo.yml"),
        ("plugin::native::piper", "piper-tts.yml"),
        ("plugin::native::matcha", "matcha-tts.yml"),
        ("plugin::native::sensevoice", "sensevoice-stt.yml"),
        ("plugin::native::nllb", "speech_to_text_translate.yml"),
    ]
    .into_iter()
    .collect();

    let filename = example_map.get(plugin_kind)?;
    let file_path = samples_dir.join(filename);

    if file_path.exists() {
        fs::read_to_string(&file_path).ok()
    } else {
        None
    }
}

fn prepend_to_library_path(dir: &Path) {
    #[cfg(target_os = "linux")]
    {
        let key = "LD_LIBRARY_PATH";
        let dir = dir.to_string_lossy();
        let existing = std::env::var_os(key).unwrap_or_default();
        let mut new_val = dir.to_string();
        if !existing.is_empty() {
            new_val.push(':');
            new_val.push_str(&existing.to_string_lossy());
        }
        std::env::set_var(key, new_val);
    }
}

fn find_official_native_plugin_artifacts(repo_root: &Path) -> Result<Vec<PathBuf>> {
    let plugins_root = repo_root.join("plugins/native");
    if !plugins_root.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(&plugins_root)
        .with_context(|| format!("failed to read {}", plugins_root.display()))?
    {
        let entry = entry?;
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }

        // Prefer release artifacts; fall back to debug if release isn't present.
        let release_dir = plugin_dir.join("target/release");
        let debug_dir = plugin_dir.join("target/debug");

        if release_dir.exists() {
            paths.extend(find_so_files(&release_dir)?);
        } else if debug_dir.exists() {
            paths.extend(find_so_files(&debug_dir)?);
        }
    }

    // Filter out debug symbol files (.d), deps folder artifacts, and proc-macro libraries
    // Keep only the main plugin library (e.g., libkokoro.so) not dependency artifacts
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    paths.retain(|p| {
        // Skip anything in deps/ directory (dependency artifacts)
        if p.components().any(|c| c.as_os_str() == "deps") {
            return false;
        }
        p.file_name()
            .and_then(|n| n.to_str())
            // Filter out debug symbol files and keep only .so/.dylib/.dll
            .is_some_and(|n| !n.ends_with(".d"))
    });
    paths.sort();
    paths.dedup();

    Ok(paths)
}

fn find_so_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        let is_native_lib = matches!(ext, Some("so" | "dylib" | "dll"));
        if is_native_lib {
            out.push(path);
        }
    }
    Ok(out)
}

/// Generate configuration reference documentation from the Config schema.
fn generate_config_docs(out_dir: &Path) -> Result<()> {
    let schema =
        serde_json::to_value(schema_for!(Config)).context("failed to generate Config schema")?;
    let defaults =
        serde_json::to_value(Config::default()).context("failed to serialize defaults")?;

    let config_path = out_dir.join("configuration-generated.md");
    let markdown = render_config_page(&schema, &defaults)?;
    write_if_changed(&config_path, &markdown)?;

    Ok(())
}

#[allow(clippy::unnecessary_wraps)] // Consistent API with other render functions
fn render_config_page(schema: &Value, defaults: &Value) -> Result<String> {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("# SPDX-FileCopyrightText: © 2025 StreamKit Contributors\n");
    out.push_str("# SPDX-License-Identifier: MPL-2.0\n");
    out.push_str("title: Configuration Reference (Generated)\n");
    out.push_str("description: Auto-generated configuration reference from schema and defaults\n");
    out.push_str("---\n\n");

    out.push_str("# Configuration Reference\n\n");
    out.push_str(
        "This page is auto-generated from the server's configuration schema and `Config::default()`. \
         For a human-friendly guide and examples, see [Configuration](./configuration/).\n\n",
    );

    // Get the properties of the root Config object
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (section_name, section_schema) in props {
            // Resolve $ref if present
            let resolved = if section_schema.get("$ref").is_some() {
                resolve_ref(schema, section_schema).unwrap_or(section_schema)
            } else {
                section_schema
            };

            let desc = resolved.get("description").and_then(Value::as_str).unwrap_or("");

            out.push_str(&format!("## `[{}]`\n\n", section_name));
            if !desc.is_empty() {
                out.push_str(desc);
                out.push_str("\n\n");
            }

            // Render properties table
            if let Some(section_props) = resolved.get("properties").and_then(Value::as_object) {
                if !section_props.is_empty() {
                    out.push_str("| Option | Type | Default | Description |\n");
                    out.push_str("|--------|------|---------|-------------|\n");

                    let defaults_section = defaults.get(section_name);
                    for (prop_name, prop_schema) in section_props {
                        let prop_resolved = resolve_ref(schema, prop_schema).unwrap_or(prop_schema);
                        let prop_type = schema_type_label(schema, prop_resolved);
                        let prop_desc =
                            prop_resolved.get("description").and_then(Value::as_str).unwrap_or("—");
                        let prop_default =
                            defaults_section.and_then(|s| s.get(prop_name)).map_or_else(
                                || "—".to_string(),
                                |v| {
                                    let s = json_one_line(v);
                                    if s.len() > 30 {
                                        format!("{}...", &s[..27])
                                    } else {
                                        s
                                    }
                                },
                            );

                        // Clean up description for table (single line)
                        let desc_single_line = prop_desc.replace('\n', " ").replace("  ", " ");

                        out.push_str(&format!(
                            "| `{}` | {} | `{}` | {} |\n",
                            prop_name,
                            prop_type,
                            prop_default.replace('|', "\\|"),
                            desc_single_line.replace('|', "\\|")
                        ));
                    }
                    out.push('\n');
                }
            }
        }
    }

    // Add raw schema
    out.push_str("## Raw JSON Schema\n\n");
    out.push_str("<details>\n");
    out.push_str("<summary>Click to expand full schema</summary>\n\n");
    out.push_str("```json\n");
    out.push_str(&serde_json::to_string_pretty(schema).unwrap_or_else(|_| "{}".to_string()));
    out.push_str("\n```\n\n");
    out.push_str("</details>\n");

    Ok(out)
}

// REUSE-IgnoreEnd
