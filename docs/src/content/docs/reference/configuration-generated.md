---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Configuration Reference (Generated)
description: Auto-generated configuration reference from schema and defaults
---

# Configuration Reference

This page is auto-generated from the server's configuration schema and `Config::default()`. For a human-friendly guide and examples, see [Configuration](./configuration/).

## `[engine]`

Engine configuration for packet processing and buffering.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `advanced` | object | `{"codec_channel_capacity":n...` | Advanced internal buffer configuration for power users. These settings affect async/blocking handoff channels in codec and container nodes. Most users should not need to modify these values. Only adjust if you understand the latency/throughput tradeoffs and have specific performance requirements. All values are in packets (not bytes). The actual memory footprint depends on packet size. |
| `node_input_capacity` | integer | null (uint) | `null` | Buffer size for node input channels (default: 128 packets) Higher = more buffering/latency, lower = more backpressure/responsiveness For low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame) |
| `oneshot` | object | `{"io_channel_capacity":null...` | Oneshot pipeline configuration (HTTP batch processing). These settings apply to stateless pipelines executed via the `/api/v1/process` endpoint. Oneshot pipelines use larger buffers by default than dynamic sessions because they don't require tight backpressure coordination. |
| `packet_batch_size` | integer (uint) | `32` | Batch size for processing packets in nodes (default: 32) Lower values = more responsive to control messages, higher values = better throughput |
| `pin_distributor_capacity` | integer | null (uint) | `null` | Buffer size between node output and pin distributor (default: 64 packets) For low-latency streaming, consider 4-8 packets |
| `profile` | null | value | `null` | Optional tuning profile that provides sensible buffering defaults. Explicit values for `node_input_capacity` and/or `pin_distributor_capacity` take precedence. |

## `[log]`

Logging configuration for console and file output.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `console_enable` | boolean | `true` | — |
| `console_level` | string enum[debug, info, warn, error] | `info` | Log level for filtering messages. |
| `file_enable` | boolean | `true` | — |
| `file_format` | string | `text` | Log file format options. |
| `file_level` | string enum[debug, info, warn, error] | `info` | Log level for filtering messages. |
| `file_path` | string | `./skit.log` | — |

## `[permissions]`

Permission configuration section for skit.toml.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allow_insecure_no_auth` | boolean | `false` | Allow starting the server on a non-loopback address without a trusted role header. StreamKit does not implement authentication; without `role_header`, all requests fall back to `SK_ROLE`/`default_role`. Binding to a non-loopback address without a trusted auth layer is unsafe and the server will refuse to start unless this flag is set. |
| `default_role` | string | `admin` | Default role for unauthenticated requests Note: StreamKit does not implement authentication by itself; this value becomes the effective role for any request that is not assigned a role by an external auth layer. For production deployments, set this to a least-privileged role and put an auth layer (or reverse proxy) in front of the server. |
| `max_concurrent_oneshots` | integer | null (uint) | `null` | Maximum concurrent oneshot pipelines (global limit) None = unlimited |
| `max_concurrent_sessions` | integer | null (uint) | `null` | Maximum concurrent dynamic sessions (global limit, applies to all users) None = unlimited |
| `role_header` | null | string | `null` | Optional trusted HTTP header used to select a role (e.g. "x-role" or "x-streamkit-role"). If unset, StreamKit ignores role headers entirely and uses `SK_ROLE`/`default_role`. Security note: Only enable this when running behind a trusted reverse proxy or auth layer that (a) authenticates the caller and (b) strips any incoming header with the same name before setting it. |
| `roles` | object | `{"admin":{"access_all_sessi...` | Map of role name -> permissions |

## `[plugins]`

Plugin directory configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allow_http_management` | boolean | `false` | Controls whether runtime plugin upload/delete is allowed via the public APIs. Default is false to avoid accidental exposure when running without an auth layer. |
| `directory` | string | `.plugins` | — |

## `[resources]`

Resource management configuration for ML models and shared resources.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `keep_models_loaded` | boolean | `true` | Keep loaded resources (models) in memory until explicit unload (default: true). When false, resources may be evicted based on LRU policy if max_memory_mb is set. |
| `max_memory_mb` | integer | null (uint) | `null` | Optional memory limit in megabytes for cached resources (models). When set, least-recently-used resources will be evicted to stay under the limit. Only applies when keep_models_loaded is false. |
| `prewarm` | object | `{"enabled":false,"plugins":[]}` | Configuration for pre-warming plugins at startup. |

## `[script]`

Configuration for the core::script node.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default_memory_limit_mb` | integer (uint) | `64` | Default memory limit for QuickJS runtime (in megabytes) |
| `default_timeout_ms` | integer (uint64) | `100` | Default timeout for script execution per packet (in milliseconds) |
| `global_fetch_allowlist` | array<object> | `[]` | Global fetch allowlist (empty = block all fetch() calls) Applies to all script nodes. Security note: there is no per-pipeline allowlist override; this prevents bypass via user-provided pipelines. |
| `secrets` | object | `{}` | Available secrets (name → environment variable mapping) Empty map = no secrets available to any script node Secrets are loaded from environment variables at server startup and can be injected into HTTP headers via pipeline configuration |

## `[security]`

Security configuration for file access and other security-sensitive settings.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allowed_file_paths` | array<string> | `["samples/**"]` | Allowed file paths for file_reader nodes. Supports glob patterns (e.g., "samples/**", "/data/media/*"). Relative paths are resolved against the server's working directory. Default: `["samples/**"]` - only allow reading from the samples directory. Set to `["**"]` to allow all paths (not recommended for production). |
| `allowed_write_paths` | array<string> | `[]` | Allowed file paths for file_writer nodes. Default: empty (deny all writes). This is intentional: arbitrary file writes from user-provided pipelines are a high-risk capability. Patterns follow the same rules as `allowed_file_paths` and are matched against the resolved absolute target path. |

## `[server]`

HTTP server configuration including TLS and CORS settings.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `address` | string | `127.0.0.1:4545` | — |
| `base_path` | null | string | `null` | Base path for subpath deployments (e.g., "/s/session_xxx"). Used to inject <base> tag in HTML. If None, no <base> tag is injected (root deployment). |
| `cert_path` | string | `` | — |
| `cors` | object | `{"allowed_origins":["http:/...` | CORS configuration for cross-origin requests. |
| `key_path` | string | `` | — |
| `max_body_size` | integer (uint) | `104857600` | Maximum request body size in bytes for multipart uploads (default: 100MB) |
| `samples_dir` | string | `./samples/pipelines` | — |
| `tls` | boolean | `false` | — |

## `[telemetry]`

Telemetry and observability configuration (OpenTelemetry, tokio-console).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | boolean | `true` | — |
| `otlp_endpoint` | null | string | `null` | — |
| `otlp_headers` | object | `{}` | — |
| `otlp_traces_endpoint` | null | string | `null` | OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`). |
| `tokio_console` | boolean | `false` | — |
| `tracing_enable` | boolean | `false` | Enable OpenTelemetry tracing (spans) export. Metrics export is controlled separately via `otlp_endpoint`. |

## Raw JSON Schema

<details>
<summary>Click to expand full schema</summary>

```json
{
  "$defs": {
    "AdvancedBufferConfig": {
      "description": "Advanced internal buffer configuration for power users.\n\nThese settings affect async/blocking handoff channels in codec and container nodes.\nMost users should not need to modify these values. Only adjust if you understand\nthe latency/throughput tradeoffs and have specific performance requirements.\n\nAll values are in packets (not bytes). The actual memory footprint depends on packet size.",
      "properties": {
        "codec_channel_capacity": {
          "description": "Capacity for codec processing channels (opus, flac, mp3) (default: 32)\nUsed for async/blocking handoff in codec nodes.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "demuxer_buffer_size": {
          "description": "Duplex buffer size for ogg demuxer in bytes (default: 65536)",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "moq_peer_channel_capacity": {
          "description": "MoQ transport peer channel capacity (default: 100)\nUsed for network send/receive coordination in MoQ transport nodes.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "stream_channel_capacity": {
          "description": "Capacity for streaming reader channels (container demuxers) (default: 8)\nSmaller than codec channels because container frames may be larger.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "AllowlistRule": {
      "description": "URL allowlist rule for fetch() API in script nodes.",
      "properties": {
        "methods": {
          "description": "Allowed HTTP methods",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "url": {
          "description": "URL pattern with wildcards (e.g., \"https://api.example.com/*\")",
          "type": "string"
        }
      },
      "required": [
        "url",
        "methods"
      ],
      "type": "object"
    },
    "CorsConfig": {
      "description": "CORS configuration for cross-origin requests.",
      "properties": {
        "allowed_origins": {
          "default": [
            "http://localhost",
            "https://localhost",
            "http://localhost:*",
            "https://localhost:*",
            "http://127.0.0.1",
            "https://127.0.0.1",
            "http://127.0.0.1:*",
            "https://127.0.0.1:*"
          ],
          "description": "Allowed origins for CORS requests.\nSupports wildcards: \"http://localhost:*\" matches any port on localhost.\nDefault: localhost and 127.0.0.1 on any port (HTTP and HTTPS).\nSet to `[\"*\"]` to allow all origins (not recommended for production).",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "type": "object"
    },
    "EngineConfig": {
      "description": "Engine configuration for packet processing and buffering.",
      "properties": {
        "advanced": {
          "$ref": "#/$defs/AdvancedBufferConfig",
          "default": {
            "codec_channel_capacity": null,
            "demuxer_buffer_size": null,
            "moq_peer_channel_capacity": null,
            "stream_channel_capacity": null
          },
          "description": "Advanced buffer tuning for codec and container nodes.\nOnly modify if you understand the latency/throughput implications."
        },
        "node_input_capacity": {
          "description": "Buffer size for node input channels (default: 128 packets)\nHigher = more buffering/latency, lower = more backpressure/responsiveness\nFor low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame)",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "oneshot": {
          "$ref": "#/$defs/OneshotConfig",
          "default": {
            "io_channel_capacity": null,
            "media_channel_capacity": null,
            "packet_batch_size": 32
          },
          "description": "Configuration for oneshot (HTTP batch) pipelines."
        },
        "packet_batch_size": {
          "default": 32,
          "description": "Batch size for processing packets in nodes (default: 32)\nLower values = more responsive to control messages, higher values = better throughput",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "pin_distributor_capacity": {
          "description": "Buffer size between node output and pin distributor (default: 64 packets)\nFor low-latency streaming, consider 4-8 packets",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "profile": {
          "anyOf": [
            {
              "$ref": "#/$defs/EnginePerfProfile"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "Optional tuning profile that provides sensible buffering defaults.\n\nExplicit values for `node_input_capacity` and/or `pin_distributor_capacity` take precedence."
        }
      },
      "type": "object"
    },
    "EnginePerfProfile": {
      "description": "Preset tuning profiles for the engine.",
      "oneOf": [
        {
          "const": "low-latency",
          "description": "Low-latency real-time streaming (minimal buffering, more backpressure)",
          "type": "string"
        },
        {
          "const": "balanced",
          "description": "Balanced defaults for general streaming and interactive pipelines",
          "type": "string"
        },
        {
          "const": "high-throughput",
          "description": "High-throughput / batch processing (more buffering, higher latency)",
          "type": "string"
        }
      ]
    },
    "LogConfig": {
      "description": "Logging configuration for console and file output.",
      "properties": {
        "console_enable": {
          "default": false,
          "type": "boolean"
        },
        "console_level": {
          "$ref": "#/$defs/LogLevel",
          "default": "info"
        },
        "file_enable": {
          "default": false,
          "type": "boolean"
        },
        "file_format": {
          "$ref": "#/$defs/LogFormat",
          "default": "text",
          "description": "Format for file logging: \"text\" (default, faster) or \"json\" (structured)"
        },
        "file_level": {
          "$ref": "#/$defs/LogLevel",
          "default": "info"
        },
        "file_path": {
          "default": "",
          "type": "string"
        }
      },
      "type": "object"
    },
    "LogFormat": {
      "description": "Log file format options.",
      "oneOf": [
        {
          "const": "text",
          "description": "Plain text format (faster, lower CPU overhead)",
          "type": "string"
        },
        {
          "const": "json",
          "description": "JSON format (structured, better for log aggregation but ~2-3x slower)",
          "type": "string"
        }
      ]
    },
    "LogLevel": {
      "description": "Log level for filtering messages.",
      "enum": [
        "debug",
        "info",
        "warn",
        "error"
      ],
      "type": "string"
    },
    "OneshotConfig": {
      "description": "Oneshot pipeline configuration (HTTP batch processing).\n\nThese settings apply to stateless pipelines executed via the `/api/v1/process` endpoint.\nOneshot pipelines use larger buffers by default than dynamic sessions because they\ndon't require tight backpressure coordination.",
      "properties": {
        "io_channel_capacity": {
          "description": "Buffer size for I/O stream channels (default: 16)\nUsed for HTTP input/output streaming.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "media_channel_capacity": {
          "description": "Buffer size for media channels between nodes (default: 256 packets)\nOneshot uses larger buffers than dynamic for batch efficiency.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "packet_batch_size": {
          "default": 32,
          "description": "Batch size for processing packets in oneshot pipelines (default: 32)\nLower values = more responsive, higher values = better throughput",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        }
      },
      "type": "object"
    },
    "Permissions": {
      "description": "Represents a set of permissions granted to a role\n\nNote: We allow excessive bools here because permissions are inherently\nindependent boolean flags. Each field represents a distinct capability\nthat can be enabled or disabled. Converting to enums or state machines\nwould complicate the API without providing meaningful benefit.\nRole-based permissions for access control.",
      "properties": {
        "access_all_sessions": {
          "default": false,
          "description": "Can access any user's sessions (admin capability)",
          "type": "boolean"
        },
        "allowed_assets": {
          "default": [],
          "description": "Allowed audio asset paths (supports globs like \"samples/audio/system/*.opus\")\nEmpty list means no assets are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "allowed_nodes": {
          "default": [],
          "description": "Allowed node types (e.g., \"audio::gain\", \"transport::moq::*\")\nEmpty list means no nodes are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "allowed_plugins": {
          "default": [],
          "description": "Allowed plugin node kinds (e.g., \"plugin::native::whisper\", \"plugin::wasm::gain\", \"plugin::*\")\nEmpty list means no plugins are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "allowed_samples": {
          "default": [],
          "description": "Allowed sample pipeline paths (supports globs like \"oneshot/*.yml\").\n\nPaths are evaluated relative to `[server].samples_dir`.\nEmpty list means no samples are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "create_sessions": {
          "default": false,
          "description": "Can create new sessions",
          "type": "boolean"
        },
        "delete_assets": {
          "default": false,
          "description": "Can delete audio assets (user assets only)",
          "type": "boolean"
        },
        "delete_plugins": {
          "default": false,
          "description": "Can delete plugins",
          "type": "boolean"
        },
        "delete_samples": {
          "default": false,
          "description": "Can delete user pipelines in `[server].samples_dir/user`",
          "type": "boolean"
        },
        "destroy_sessions": {
          "default": false,
          "description": "Can destroy sessions (their own or any depending on context)",
          "type": "boolean"
        },
        "list_nodes": {
          "default": false,
          "description": "Can view the list of available nodes",
          "type": "boolean"
        },
        "list_samples": {
          "default": false,
          "description": "Can list sample pipelines",
          "type": "boolean"
        },
        "list_sessions": {
          "default": false,
          "description": "Can list sessions (their own or all depending on context)",
          "type": "boolean"
        },
        "load_plugins": {
          "default": false,
          "description": "Can upload and load plugins (WASM or native)",
          "type": "boolean"
        },
        "modify_sessions": {
          "default": false,
          "description": "Can modify running sessions (add/remove nodes)",
          "type": "boolean"
        },
        "read_samples": {
          "default": false,
          "description": "Can read sample pipeline YAML",
          "type": "boolean"
        },
        "tune_nodes": {
          "default": false,
          "description": "Can tune parameters on running nodes",
          "type": "boolean"
        },
        "upload_assets": {
          "default": false,
          "description": "Can upload audio assets",
          "type": "boolean"
        },
        "write_samples": {
          "default": false,
          "description": "Can save/update user pipelines in `[server].samples_dir/user`",
          "type": "boolean"
        }
      },
      "type": "object"
    },
    "PermissionsConfig": {
      "description": "Permission configuration section for skit.toml.",
      "properties": {
        "allow_insecure_no_auth": {
          "default": false,
          "description": "Allow starting the server on a non-loopback address without a trusted role header.\n\nStreamKit does not implement authentication; without `role_header`, all requests fall back to\n`SK_ROLE`/`default_role`. Binding to a non-loopback address without a trusted auth layer is\nunsafe and the server will refuse to start unless this flag is set.",
          "type": "boolean"
        },
        "default_role": {
          "default": "admin",
          "description": "Default role for unauthenticated requests\n\nNote: StreamKit does not implement authentication by itself; this value becomes the\neffective role for any request that is not assigned a role by an external auth layer.\nFor production deployments, set this to a least-privileged role and put an auth layer\n(or reverse proxy) in front of the server.",
          "type": "string"
        },
        "max_concurrent_oneshots": {
          "default": null,
          "description": "Maximum concurrent oneshot pipelines (global limit)\nNone = unlimited",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "max_concurrent_sessions": {
          "default": null,
          "description": "Maximum concurrent dynamic sessions (global limit, applies to all users)\nNone = unlimited",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "role_header": {
          "default": null,
          "description": "Optional trusted HTTP header used to select a role (e.g. \"x-role\" or \"x-streamkit-role\").\n\nIf unset, StreamKit ignores role headers entirely and uses `SK_ROLE`/`default_role`.\n\nSecurity note: Only enable this when running behind a trusted reverse proxy or\nauth layer that (a) authenticates the caller and (b) strips any incoming header\nwith the same name before setting it.",
          "type": [
            "string",
            "null"
          ]
        },
        "roles": {
          "additionalProperties": {
            "$ref": "#/$defs/Permissions"
          },
          "default": {
            "admin": {
              "access_all_sessions": true,
              "allowed_assets": [
                "*"
              ],
              "allowed_nodes": [
                "*"
              ],
              "allowed_plugins": [
                "*"
              ],
              "allowed_samples": [
                "*"
              ],
              "create_sessions": true,
              "delete_assets": true,
              "delete_plugins": true,
              "delete_samples": true,
              "destroy_sessions": true,
              "list_nodes": true,
              "list_samples": true,
              "list_sessions": true,
              "load_plugins": true,
              "modify_sessions": true,
              "read_samples": true,
              "tune_nodes": true,
              "upload_assets": true,
              "write_samples": true
            },
            "user": {
              "access_all_sessions": false,
              "allowed_assets": [
                "samples/audio/system/*",
                "samples/audio/user/*"
              ],
              "allowed_nodes": [
                "audio::*",
                "containers::*",
                "transport::moq::*",
                "core::passthrough",
                "core::file_reader",
                "core::pacer",
                "core::json_serialize",
                "core::text_chunker",
                "core::script",
                "core::telemetry_tap",
                "core::telemetry_out",
                "core::sink"
              ],
              "allowed_plugins": [
                "plugin::*",
                "native::audio*"
              ],
              "allowed_samples": [
                "oneshot/*.yml",
                "oneshot/*.yaml",
                "dynamic/*.yml",
                "dynamic/*.yaml",
                "user/*.yml",
                "user/*.yaml"
              ],
              "create_sessions": true,
              "delete_assets": true,
              "delete_plugins": false,
              "delete_samples": true,
              "destroy_sessions": true,
              "list_nodes": true,
              "list_samples": true,
              "list_sessions": true,
              "load_plugins": false,
              "modify_sessions": true,
              "read_samples": true,
              "tune_nodes": true,
              "upload_assets": true,
              "write_samples": true
            }
          },
          "description": "Map of role name -> permissions",
          "type": "object"
        }
      },
      "type": "object"
    },
    "PluginConfig": {
      "description": "Plugin directory configuration.",
      "properties": {
        "allow_http_management": {
          "default": false,
          "description": "Controls whether runtime plugin upload/delete is allowed via the public APIs.\n\nDefault is false to avoid accidental exposure when running without an auth layer.",
          "type": "boolean"
        },
        "directory": {
          "type": "string"
        }
      },
      "required": [
        "directory"
      ],
      "type": "object"
    },
    "PrewarmConfig": {
      "description": "Configuration for pre-warming plugins at startup.",
      "properties": {
        "enabled": {
          "default": false,
          "description": "Enable pre-warming (default: false)",
          "type": "boolean"
        },
        "plugins": {
          "default": [],
          "description": "List of plugins to pre-warm with their parameters",
          "items": {
            "$ref": "#/$defs/PrewarmPluginConfig"
          },
          "type": "array"
        }
      },
      "type": "object"
    },
    "PrewarmPluginConfig": {
      "description": "Configuration for a single plugin to pre-warm at startup.",
      "properties": {
        "fallback_params": {
          "default": null,
          "description": "Optional fallback parameters to try if the primary params fail\nUseful for GPU plugins that should fallback to CPU"
        },
        "kind": {
          "description": "Plugin kind (e.g., \"plugin::native::kokoro\", \"plugin::native::whisper\")",
          "type": "string"
        },
        "params": {
          "default": null,
          "description": "Parameters to use when creating the warmup instance\nThese should match the most common usage pattern"
        }
      },
      "required": [
        "kind"
      ],
      "type": "object"
    },
    "ResourceConfig": {
      "description": "Resource management configuration for ML models and shared resources.",
      "properties": {
        "keep_models_loaded": {
          "default": true,
          "description": "Keep loaded resources (models) in memory until explicit unload (default: true).\nWhen false, resources may be evicted based on LRU policy if max_memory_mb is set.",
          "type": "boolean"
        },
        "max_memory_mb": {
          "description": "Optional memory limit in megabytes for cached resources (models).\nWhen set, least-recently-used resources will be evicted to stay under the limit.\nOnly applies when keep_models_loaded is false.",
          "format": "uint",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "prewarm": {
          "$ref": "#/$defs/PrewarmConfig",
          "default": {
            "enabled": false,
            "plugins": []
          },
          "description": "Pre-warming configuration for reducing first-use latency"
        }
      },
      "type": "object"
    },
    "ScriptConfig": {
      "description": "Configuration for the core::script node.",
      "properties": {
        "default_memory_limit_mb": {
          "default": 64,
          "description": "Default memory limit for QuickJS runtime (in megabytes)",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "default_timeout_ms": {
          "default": 100,
          "description": "Default timeout for script execution per packet (in milliseconds)",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "global_fetch_allowlist": {
          "default": [],
          "description": "Global fetch allowlist (empty = block all fetch() calls)\nApplies to all script nodes.\n\nSecurity note: there is no per-pipeline allowlist override; this prevents bypass via\nuser-provided pipelines.",
          "items": {
            "$ref": "#/$defs/AllowlistRule"
          },
          "type": "array"
        },
        "secrets": {
          "additionalProperties": {
            "$ref": "#/$defs/SecretConfig"
          },
          "default": {},
          "description": "Available secrets (name → environment variable mapping)\nEmpty map = no secrets available to any script node\nSecrets are loaded from environment variables at server startup\nand can be injected into HTTP headers via pipeline configuration",
          "type": "object"
        }
      },
      "type": "object"
    },
    "SecretConfig": {
      "description": "Configuration for a single secret loaded from environment.",
      "properties": {
        "allowed_fetch_urls": {
          "default": [],
          "description": "Optional allowlist of URL patterns where this secret may be injected into `fetch()` headers.\n\nPatterns use the same format as `script.global_fetch_allowlist` entries:\n- `https://api.openai.com/*`\n- `https://api.openai.com/v1/chat/completions`\n\nEmpty = no additional restriction (backwards-compatible).",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "description": {
          "default": "",
          "description": "Optional description for documentation",
          "type": "string"
        },
        "env": {
          "description": "Environment variable name containing the secret value",
          "type": "string"
        },
        "type": {
          "$ref": "#/$defs/SecretType",
          "default": "string",
          "description": "Type of secret (for validation and formatting)"
        }
      },
      "required": [
        "env"
      ],
      "type": "object"
    },
    "SecretType": {
      "description": "Type of secret for validation and documentation.",
      "oneOf": [
        {
          "const": "url",
          "description": "URL (e.g., webhook URLs)",
          "type": "string"
        },
        {
          "const": "token",
          "description": "Bearer token",
          "type": "string"
        },
        {
          "const": "apikey",
          "description": "API key",
          "type": "string"
        },
        {
          "const": "string",
          "description": "Generic string",
          "type": "string"
        }
      ]
    },
    "SecurityConfig": {
      "description": "Security configuration for file access and other security-sensitive settings.",
      "properties": {
        "allowed_file_paths": {
          "default": [
            "samples/**"
          ],
          "description": "Allowed file paths for file_reader nodes.\nSupports glob patterns (e.g., \"samples/**\", \"/data/media/*\").\nRelative paths are resolved against the server's working directory.\nDefault: `[\"samples/**\"]` - only allow reading from the samples directory.\nSet to `[\"**\"]` to allow all paths (not recommended for production).",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "allowed_write_paths": {
          "default": [],
          "description": "Allowed file paths for file_writer nodes.\n\nDefault: empty (deny all writes). This is intentional: arbitrary file writes from\nuser-provided pipelines are a high-risk capability.\n\nPatterns follow the same rules as `allowed_file_paths` and are matched against the\nresolved absolute target path.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "type": "object"
    },
    "ServerConfig": {
      "description": "HTTP server configuration including TLS and CORS settings.",
      "properties": {
        "address": {
          "type": "string"
        },
        "base_path": {
          "description": "Base path for subpath deployments (e.g., \"/s/session_xxx\"). Used to inject <base> tag in HTML.\nIf None, no <base> tag is injected (root deployment).",
          "type": [
            "string",
            "null"
          ]
        },
        "cert_path": {
          "type": "string"
        },
        "cors": {
          "$ref": "#/$defs/CorsConfig",
          "default": {
            "allowed_origins": [
              "http://localhost",
              "https://localhost",
              "http://localhost:*",
              "https://localhost:*",
              "http://127.0.0.1",
              "https://127.0.0.1",
              "http://127.0.0.1:*",
              "https://127.0.0.1:*"
            ]
          },
          "description": "CORS configuration for cross-origin requests"
        },
        "key_path": {
          "type": "string"
        },
        "max_body_size": {
          "default": 104857600,
          "description": "Maximum request body size in bytes for multipart uploads (default: 100MB)",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "samples_dir": {
          "type": "string"
        },
        "tls": {
          "type": "boolean"
        }
      },
      "required": [
        "address",
        "tls",
        "cert_path",
        "key_path",
        "samples_dir"
      ],
      "type": "object"
    },
    "TelemetryConfig": {
      "description": "Telemetry and observability configuration (OpenTelemetry, tokio-console).",
      "properties": {
        "enable": {
          "default": true,
          "type": "boolean"
        },
        "otlp_endpoint": {
          "type": [
            "string",
            "null"
          ]
        },
        "otlp_headers": {
          "additionalProperties": {
            "type": "string"
          },
          "default": {},
          "type": "object"
        },
        "otlp_traces_endpoint": {
          "description": "OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`).",
          "type": [
            "string",
            "null"
          ]
        },
        "tokio_console": {
          "default": false,
          "type": "boolean"
        },
        "tracing_enable": {
          "default": false,
          "description": "Enable OpenTelemetry tracing (spans) export.\n\nMetrics export is controlled separately via `otlp_endpoint`.",
          "type": "boolean"
        }
      },
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Root configuration for the StreamKit server.",
  "properties": {
    "engine": {
      "$ref": "#/$defs/EngineConfig",
      "default": {
        "advanced": {
          "codec_channel_capacity": null,
          "demuxer_buffer_size": null,
          "moq_peer_channel_capacity": null,
          "stream_channel_capacity": null
        },
        "node_input_capacity": null,
        "oneshot": {
          "io_channel_capacity": null,
          "media_channel_capacity": null,
          "packet_batch_size": 32
        },
        "packet_batch_size": 32,
        "pin_distributor_capacity": null,
        "profile": null
      }
    },
    "log": {
      "$ref": "#/$defs/LogConfig",
      "default": {
        "console_enable": true,
        "console_level": "info",
        "file_enable": true,
        "file_format": "text",
        "file_level": "info",
        "file_path": "./skit.log"
      }
    },
    "permissions": {
      "$ref": "#/$defs/PermissionsConfig",
      "default": {
        "allow_insecure_no_auth": false,
        "default_role": "admin",
        "max_concurrent_oneshots": null,
        "max_concurrent_sessions": null,
        "role_header": null,
        "roles": {
          "admin": {
            "access_all_sessions": true,
            "allowed_assets": [
              "*"
            ],
            "allowed_nodes": [
              "*"
            ],
            "allowed_plugins": [
              "*"
            ],
            "allowed_samples": [
              "*"
            ],
            "create_sessions": true,
            "delete_assets": true,
            "delete_plugins": true,
            "delete_samples": true,
            "destroy_sessions": true,
            "list_nodes": true,
            "list_samples": true,
            "list_sessions": true,
            "load_plugins": true,
            "modify_sessions": true,
            "read_samples": true,
            "tune_nodes": true,
            "upload_assets": true,
            "write_samples": true
          },
          "user": {
            "access_all_sessions": false,
            "allowed_assets": [
              "samples/audio/system/*",
              "samples/audio/user/*"
            ],
            "allowed_nodes": [
              "audio::*",
              "containers::*",
              "transport::moq::*",
              "core::passthrough",
              "core::file_reader",
              "core::pacer",
              "core::json_serialize",
              "core::text_chunker",
              "core::script",
              "core::telemetry_tap",
              "core::telemetry_out",
              "core::sink"
            ],
            "allowed_plugins": [
              "plugin::*",
              "native::audio*"
            ],
            "allowed_samples": [
              "oneshot/*.yml",
              "oneshot/*.yaml",
              "dynamic/*.yml",
              "dynamic/*.yaml",
              "user/*.yml",
              "user/*.yaml"
            ],
            "create_sessions": true,
            "delete_assets": true,
            "delete_plugins": false,
            "delete_samples": true,
            "destroy_sessions": true,
            "list_nodes": true,
            "list_samples": true,
            "list_sessions": true,
            "load_plugins": false,
            "modify_sessions": true,
            "read_samples": true,
            "tune_nodes": true,
            "upload_assets": true,
            "write_samples": true
          }
        }
      }
    },
    "plugins": {
      "$ref": "#/$defs/PluginConfig",
      "default": {
        "allow_http_management": false,
        "directory": ".plugins"
      }
    },
    "resources": {
      "$ref": "#/$defs/ResourceConfig",
      "default": {
        "keep_models_loaded": true,
        "max_memory_mb": null,
        "prewarm": {
          "enabled": false,
          "plugins": []
        }
      }
    },
    "script": {
      "$ref": "#/$defs/ScriptConfig",
      "default": {
        "default_memory_limit_mb": 64,
        "default_timeout_ms": 100,
        "global_fetch_allowlist": [],
        "secrets": {}
      }
    },
    "security": {
      "$ref": "#/$defs/SecurityConfig",
      "default": {
        "allowed_file_paths": [
          "samples/**"
        ],
        "allowed_write_paths": []
      }
    },
    "server": {
      "$ref": "#/$defs/ServerConfig",
      "default": {
        "address": "127.0.0.1:4545",
        "base_path": null,
        "cert_path": "",
        "cors": {
          "allowed_origins": [
            "http://localhost",
            "https://localhost",
            "http://localhost:*",
            "https://localhost:*",
            "http://127.0.0.1",
            "https://127.0.0.1",
            "http://127.0.0.1:*",
            "https://127.0.0.1:*"
          ]
        },
        "key_path": "",
        "max_body_size": 104857600,
        "samples_dir": "./samples/pipelines",
        "tls": false
      }
    },
    "telemetry": {
      "$ref": "#/$defs/TelemetryConfig",
      "default": {
        "enable": true,
        "otlp_endpoint": null,
        "otlp_headers": {},
        "otlp_traces_endpoint": null,
        "tokio_console": false,
        "tracing_enable": false
      }
    }
  },
  "title": "Config",
  "type": "object"
}
```

</details>
