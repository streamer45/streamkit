---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Configuration Reference (Generated)
description: Auto-generated configuration reference from schema and defaults
---

# Configuration Reference

This page is auto-generated from the server's configuration schema and `Config::default()`. For a human-friendly guide and examples, see [Configuration](./configuration/).

## `[server]`

HTTP server configuration including TLS and CORS settings.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `address` | string | `127.0.0.1:4545` | — |
| `tls` | boolean | `false` | — |
| `cert_path` | string | `` | — |
| `key_path` | string | `` | — |
| `samples_dir` | string | `./samples/pipelines` | — |
| `max_body_size` | integer (uint) | `104857600` | Maximum request body size in bytes for multipart uploads (default: 100MB) |
| `base_path` | null | string | `null` | Base path for subpath deployments (e.g., "/s/session_xxx"). Used to inject <base> tag in HTML. If None, no <base> tag is injected (root deployment). |
| `cors` | object | `{"allowed_origins":["http:/...` | CORS configuration for cross-origin requests. |

## `[security]`

Security configuration for file access and other security-sensitive settings.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allowed_file_paths` | array<string> | `["samples/**"]` | Allowed file paths for file_reader nodes. Supports glob patterns (e.g., "samples/**", "/data/media/*"). Relative paths are resolved against the server's working directory. Default: `["samples/**"]` - only allow reading from the samples directory. Set to `["**"]` to allow all paths (not recommended for production). |
| `allowed_write_paths` | array<string> | `[]` | Allowed file paths for file_writer nodes. Default: empty (deny all writes). This is intentional: arbitrary file writes from user-provided pipelines are a high-risk capability. Patterns follow the same rules as `allowed_file_paths` and are matched against the resolved absolute target path. |

## `[log]`

Logging configuration for console and file output.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `console_enable` | boolean | `true` | — |
| `file_enable` | boolean | `true` | — |
| `console_level` | string enum[debug, info, warn, error] | `info` | Log level for filtering messages. |
| `file_level` | string enum[debug, info, warn, error] | `info` | Log level for filtering messages. |
| `file_path` | string | `./skit.log` | — |
| `file_format` | string | `text` | Log file format options. |

## `[telemetry]`

Telemetry and observability configuration (OpenTelemetry, tokio-console).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | boolean | `true` | — |
| `tracing_enable` | boolean | `false` | Enable OpenTelemetry tracing (spans) export. Metrics export is controlled separately via `otlp_endpoint`. |
| `otlp_endpoint` | null | string | `null` | — |
| `otlp_traces_endpoint` | null | string | `null` | OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`). |
| `otlp_headers` | object | `{}` | — |
| `tokio_console` | boolean | `false` | — |

## `[engine]`

Engine configuration for packet processing and buffering.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `profile` | null | value | `null` | Optional tuning profile that provides sensible buffering defaults. Explicit values for `node_input_capacity` and/or `pin_distributor_capacity` take precedence. |
| `packet_batch_size` | integer (uint) | `32` | Batch size for processing packets in nodes (default: 32) Lower values = more responsive to control messages, higher values = better throughput |
| `node_input_capacity` | integer | null (uint) | `null` | Buffer size for node input channels (default: 128 packets) Higher = more buffering/latency, lower = more backpressure/responsiveness For low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame) |
| `pin_distributor_capacity` | integer | null (uint) | `null` | Buffer size between node output and pin distributor (default: 64 packets) For low-latency streaming, consider 4-8 packets |
| `oneshot` | object | `{"packet_batch_size":32,"me...` | Oneshot pipeline configuration (HTTP batch processing). These settings apply to stateless pipelines executed via the `/api/v1/process` endpoint. Oneshot pipelines use larger buffers by default than dynamic sessions because they don't require tight backpressure coordination. |
| `advanced` | object | `{"codec_channel_capacity":n...` | Advanced internal buffer configuration for power users. These settings affect async/blocking handoff channels in codec and container nodes. Most users should not need to modify these values. Only adjust if you understand the latency/throughput tradeoffs and have specific performance requirements. All values are in packets (not bytes). The actual memory footprint depends on packet size. |

## `[plugins]`

Plugin directory configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `directory` | string | `.plugins` | — |
| `allow_http_management` | boolean | `false` | Controls whether runtime plugin upload/delete is allowed via the public APIs. Default is false to avoid accidental exposure when running without an auth layer. |
| `marketplace_enabled` | boolean | `false` | Enables the plugin marketplace API and UI (default: false). |
| `allow_native_marketplace` | boolean | `false` | Allows native plugins to be installed from a marketplace (default: false). Native plugins run in-process and are unsafe without full trust. |
| `allow_model_urls` | boolean | `false` | Allow direct URL model downloads from manifests (default: false). |
| `marketplace_require_registry_origin` | boolean | `false` | Require marketplace URLs to share origin with the registry (default: false). |
| `marketplace_scheme_policy` | string enum[https_only, allow_http] | `https_only` | — |
| `marketplace_host_policy` | string enum[public_only, allow_private] | `public_only` | — |
| `marketplace_resolve_hostnames` | boolean | `false` | Resolve hostnames for marketplace URLs and check resolved IPs (default: false). |
| `marketplace_url_allowlist` | array<string> | `[]` | Allowed marketplace origins (e.g., "https://example.com", "https://example.com:*"). |
| `trusted_pubkeys` | array<string> | `[]` | Minisign public keys (contents of `.pub` files) trusted for marketplace manifests. |
| `registries` | array<string> | `[]` | Registry index URLs (e.g., `https://example.com/index.json`). |
| `models_dir` | null | string | `null` | Optional directory to store downloaded models (defaults to `models` when unset). |
| `huggingface_token` | null | string | `null` | Optional Hugging Face token for gated model downloads. |

## `[resources]`

Resource management configuration for ML models and shared resources.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `keep_models_loaded` | boolean | `true` | Keep loaded resources (models) in memory until explicit unload (default: true). When false, resources may be evicted based on LRU policy if max_memory_mb is set. |
| `max_memory_mb` | integer | null (uint) | `null` | Optional memory limit in megabytes for cached resources (models). When set, least-recently-used resources will be evicted to stay under the limit. Only applies when keep_models_loaded is false. |
| `prewarm` | object | `{"enabled":false,"plugins":[]}` | Configuration for pre-warming plugins at startup. |

## `[permissions]`

Permission configuration section for skit.toml.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default_role` | string | `admin` | Default role for requests without an authenticated role When built-in auth is disabled, this becomes the effective role for requests that are not assigned a role via a trusted role header or `SK_ROLE`. For production deployments, prefer enabling built-in auth (`[auth].mode`) or running behind an authenticating reverse proxy that sets `[permissions].role_header`. |
| `role_header` | null | string | `null` | Optional trusted HTTP header used to select a role (e.g. "x-role" or "x-streamkit-role"). If unset, StreamKit ignores role headers entirely and uses `SK_ROLE`/`default_role`. Security note: Only enable this when running behind a trusted reverse proxy or auth layer that (a) authenticates the caller and (b) strips any incoming header with the same name before setting it. |
| `allow_insecure_no_auth` | boolean | `false` | Allow starting the server on a non-loopback address without built-in auth or a trusted role header. This only applies when built-in auth is disabled. This is unsafe: all requests fall back to `SK_ROLE`/`default_role`. The server refuses to start in this configuration unless this flag is set. |
| `roles` | object | `{"user":{"create_sessions":...` | Map of role name -> permissions |
| `max_concurrent_sessions` | integer | null (uint) | `null` | Maximum concurrent dynamic sessions (global limit, applies to all users) None = unlimited |
| `max_concurrent_oneshots` | integer | null (uint) | `null` | Maximum concurrent oneshot pipelines (global limit) None = unlimited |

## `[script]`

Configuration for the core::script node.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default_timeout_ms` | integer (uint64) | `100` | Default timeout for script execution per packet (in milliseconds) |
| `default_memory_limit_mb` | integer (uint) | `64` | Default memory limit for QuickJS runtime (in megabytes) |
| `global_fetch_allowlist` | array<object> | `[]` | Global fetch allowlist (empty = block all fetch() calls) Applies to all script nodes. Security note: there is no per-pipeline allowlist override; this prevents bypass via user-provided pipelines. |
| `secrets` | object | `{}` | Available secrets (name → environment variable mapping) Empty map = no secrets available to any script node Secrets are loaded from environment variables at server startup and can be injected into HTTP headers via pipeline configuration |

## `[compositor]`

Server-level defaults for the video compositor node.

These limits apply to every compositor node created by the engine.
Individual nodes cannot exceed these values, even via `UpdateParams`.

```toml
[compositor]
max_canvas_dimension = 7680
max_font_size = 4096
max_text_length = 10000
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `max_canvas_dimension` | integer (uint32) | `7680` | Maximum allowed canvas dimension (width or height) in pixels. Default: 7680 (8K UHD). |
| `max_font_size` | integer (uint32) | `4096` | Maximum allowed font size for text overlays in pixels. Default: 4096. |
| `max_text_length` | integer (uint) | `10000` | Maximum allowed text overlay string length in bytes. Default: 10000. |

## `[auth]`

Authentication configuration for built-in JWT-based auth.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `mode` | string | `auto` | Authentication mode for the server. |
| `state_dir` | string | `.streamkit/auth` | Directory for auth state (keys, tokens). Default: ".streamkit/auth" |
| `cookie_name` | string | `skit_session` | Cookie name for browser sessions. Default: "skit_session" |
| `api_default_ttl_secs` | integer (uint64) | `86400` | Default TTL for API tokens in seconds. Default: 86400 (24 hours) |
| `api_max_ttl_secs` | integer (uint64) | `2592000` | Maximum TTL for API tokens in seconds. Default: 2592000 (30 days) |
| `moq_default_ttl_secs` | integer (uint64) | `3600` | Default TTL for MoQ tokens in seconds. Default: 3600 (1 hour) |
| `moq_max_ttl_secs` | integer (uint64) | `86400` | Maximum TTL for MoQ tokens in seconds. Default: 86400 (1 day) |

## Raw JSON Schema

<details>
<summary>Click to expand full schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Config",
  "description": "Root configuration for the StreamKit server.",
  "type": "object",
  "properties": {
    "server": {
      "$ref": "#/$defs/ServerConfig",
      "default": {
        "address": "127.0.0.1:4545",
        "tls": false,
        "cert_path": "",
        "key_path": "",
        "samples_dir": "./samples/pipelines",
        "max_body_size": 104857600,
        "base_path": null,
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
        }
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
    "log": {
      "$ref": "#/$defs/LogConfig",
      "default": {
        "console_enable": true,
        "file_enable": true,
        "console_level": "info",
        "file_level": "info",
        "file_path": "./skit.log",
        "file_format": "text"
      }
    },
    "telemetry": {
      "$ref": "#/$defs/TelemetryConfig",
      "default": {
        "enable": true,
        "tracing_enable": false,
        "otlp_endpoint": null,
        "otlp_traces_endpoint": null,
        "otlp_headers": {},
        "tokio_console": false
      }
    },
    "engine": {
      "$ref": "#/$defs/EngineConfig",
      "default": {
        "profile": null,
        "packet_batch_size": 32,
        "node_input_capacity": null,
        "pin_distributor_capacity": null,
        "oneshot": {
          "packet_batch_size": 32,
          "media_channel_capacity": null,
          "io_channel_capacity": null
        },
        "advanced": {
          "codec_channel_capacity": null,
          "stream_channel_capacity": null,
          "demuxer_buffer_size": null,
          "moq_peer_channel_capacity": null
        }
      }
    },
    "plugins": {
      "$ref": "#/$defs/PluginConfig",
      "default": {
        "directory": ".plugins",
        "allow_http_management": false,
        "marketplace_enabled": false,
        "allow_native_marketplace": false,
        "allow_model_urls": false,
        "marketplace_require_registry_origin": false,
        "marketplace_scheme_policy": "https_only",
        "marketplace_host_policy": "public_only",
        "marketplace_resolve_hostnames": false,
        "marketplace_url_allowlist": [],
        "trusted_pubkeys": [],
        "registries": [],
        "models_dir": null,
        "huggingface_token": null
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
    "permissions": {
      "$ref": "#/$defs/PermissionsConfig",
      "default": {
        "default_role": "admin",
        "role_header": null,
        "allow_insecure_no_auth": false,
        "roles": {
          "admin": {
            "create_sessions": true,
            "destroy_sessions": true,
            "list_sessions": true,
            "modify_sessions": true,
            "tune_nodes": true,
            "load_plugins": true,
            "delete_plugins": true,
            "list_nodes": true,
            "list_samples": true,
            "read_samples": true,
            "write_samples": true,
            "delete_samples": true,
            "allowed_samples": [
              "*"
            ],
            "allowed_nodes": [
              "*"
            ],
            "allowed_plugins": [
              "*"
            ],
            "access_all_sessions": true,
            "upload_assets": true,
            "delete_assets": true,
            "allowed_assets": [
              "*"
            ]
          },
          "viewer": {
            "create_sessions": false,
            "destroy_sessions": false,
            "list_sessions": true,
            "modify_sessions": false,
            "tune_nodes": false,
            "load_plugins": false,
            "delete_plugins": false,
            "list_nodes": true,
            "list_samples": true,
            "read_samples": true,
            "write_samples": false,
            "delete_samples": false,
            "allowed_samples": [
              "oneshot/*.yml",
              "oneshot/*.yaml",
              "dynamic/*.yml",
              "dynamic/*.yaml",
              "user/*.yml",
              "user/*.yaml"
            ],
            "allowed_nodes": [
              "*"
            ],
            "allowed_plugins": [
              "*"
            ],
            "access_all_sessions": false,
            "upload_assets": false,
            "delete_assets": false,
            "allowed_assets": [
              "samples/audio/system/*"
            ]
          },
          "user": {
            "create_sessions": true,
            "destroy_sessions": true,
            "list_sessions": true,
            "modify_sessions": true,
            "tune_nodes": true,
            "load_plugins": false,
            "delete_plugins": false,
            "list_nodes": true,
            "list_samples": true,
            "read_samples": true,
            "write_samples": true,
            "delete_samples": true,
            "allowed_samples": [
              "oneshot/*.yml",
              "oneshot/*.yaml",
              "dynamic/*.yml",
              "dynamic/*.yaml",
              "user/*.yml",
              "user/*.yaml"
            ],
            "allowed_nodes": [
              "audio::*",
              "video::*",
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
              "core::sink",
              "plugin::*"
            ],
            "allowed_plugins": [
              "plugin::*"
            ],
            "access_all_sessions": false,
            "upload_assets": true,
            "delete_assets": true,
            "allowed_assets": [
              "samples/audio/system/*",
              "samples/audio/user/*"
            ]
          }
        },
        "max_concurrent_sessions": null,
        "max_concurrent_oneshots": null
      }
    },
    "script": {
      "$ref": "#/$defs/ScriptConfig",
      "default": {
        "default_timeout_ms": 100,
        "default_memory_limit_mb": 64,
        "global_fetch_allowlist": [],
        "secrets": {}
      }
    },
    "compositor": {
      "$ref": "#/$defs/CompositorServerConfig",
      "default": {
        "max_canvas_dimension": 7680,
        "max_font_size": 4096,
        "max_text_length": 10000
      }
    },
    "auth": {
      "$ref": "#/$defs/AuthConfig",
      "default": {
        "mode": "auto",
        "state_dir": ".streamkit/auth",
        "cookie_name": "skit_session",
        "api_default_ttl_secs": 86400,
        "api_max_ttl_secs": 2592000,
        "moq_default_ttl_secs": 3600,
        "moq_max_ttl_secs": 86400
      }
    }
  },
  "$defs": {
    "ServerConfig": {
      "description": "HTTP server configuration including TLS and CORS settings.",
      "type": "object",
      "properties": {
        "address": {
          "type": "string"
        },
        "tls": {
          "type": "boolean"
        },
        "cert_path": {
          "type": "string"
        },
        "key_path": {
          "type": "string"
        },
        "samples_dir": {
          "type": "string"
        },
        "max_body_size": {
          "description": "Maximum request body size in bytes for multipart uploads (default: 100MB)",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 104857600
        },
        "base_path": {
          "description": "Base path for subpath deployments (e.g., \"/s/session_xxx\"). Used to inject <base> tag in HTML.\nIf None, no <base> tag is injected (root deployment).",
          "type": [
            "string",
            "null"
          ]
        },
        "cors": {
          "description": "CORS configuration for cross-origin requests",
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
          }
        }
      },
      "required": [
        "address",
        "tls",
        "cert_path",
        "key_path",
        "samples_dir"
      ]
    },
    "CorsConfig": {
      "description": "CORS configuration for cross-origin requests.",
      "type": "object",
      "properties": {
        "allowed_origins": {
          "description": "Allowed origins for CORS requests.\nSupports wildcards: \"http://localhost:*\" matches any port on localhost.\nDefault: localhost and 127.0.0.1 on any port (HTTP and HTTPS).\nSet to `[\"*\"]` to allow all origins (not recommended for production).",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": [
            "http://localhost",
            "https://localhost",
            "http://localhost:*",
            "https://localhost:*",
            "http://127.0.0.1",
            "https://127.0.0.1",
            "http://127.0.0.1:*",
            "https://127.0.0.1:*"
          ]
        }
      }
    },
    "SecurityConfig": {
      "description": "Security configuration for file access and other security-sensitive settings.",
      "type": "object",
      "properties": {
        "allowed_file_paths": {
          "description": "Allowed file paths for file_reader nodes.\nSupports glob patterns (e.g., \"samples/**\", \"/data/media/*\").\nRelative paths are resolved against the server's working directory.\nDefault: `[\"samples/**\"]` - only allow reading from the samples directory.\nSet to `[\"**\"]` to allow all paths (not recommended for production).",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": [
            "samples/**"
          ]
        },
        "allowed_write_paths": {
          "description": "Allowed file paths for file_writer nodes.\n\nDefault: empty (deny all writes). This is intentional: arbitrary file writes from\nuser-provided pipelines are a high-risk capability.\n\nPatterns follow the same rules as `allowed_file_paths` and are matched against the\nresolved absolute target path.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        }
      }
    },
    "LogConfig": {
      "description": "Logging configuration for console and file output.",
      "type": "object",
      "properties": {
        "console_enable": {
          "type": "boolean",
          "default": false
        },
        "file_enable": {
          "type": "boolean",
          "default": false
        },
        "console_level": {
          "$ref": "#/$defs/LogLevel",
          "default": "info"
        },
        "file_level": {
          "$ref": "#/$defs/LogLevel",
          "default": "info"
        },
        "file_path": {
          "type": "string",
          "default": ""
        },
        "file_format": {
          "description": "Format for file logging: \"text\" (default, faster) or \"json\" (structured)",
          "$ref": "#/$defs/LogFormat",
          "default": "text"
        }
      }
    },
    "LogLevel": {
      "description": "Log level for filtering messages.",
      "type": "string",
      "enum": [
        "debug",
        "info",
        "warn",
        "error"
      ]
    },
    "LogFormat": {
      "description": "Log file format options.",
      "oneOf": [
        {
          "description": "Plain text format (faster, lower CPU overhead)",
          "type": "string",
          "const": "text"
        },
        {
          "description": "JSON format (structured, better for log aggregation but ~2-3x slower)",
          "type": "string",
          "const": "json"
        }
      ]
    },
    "TelemetryConfig": {
      "description": "Telemetry and observability configuration (OpenTelemetry, tokio-console).",
      "type": "object",
      "properties": {
        "enable": {
          "type": "boolean",
          "default": true
        },
        "tracing_enable": {
          "description": "Enable OpenTelemetry tracing (spans) export.\n\nMetrics export is controlled separately via `otlp_endpoint`.",
          "type": "boolean",
          "default": false
        },
        "otlp_endpoint": {
          "type": [
            "string",
            "null"
          ]
        },
        "otlp_traces_endpoint": {
          "description": "OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`).",
          "type": [
            "string",
            "null"
          ]
        },
        "otlp_headers": {
          "type": "object",
          "additionalProperties": {
            "type": "string"
          },
          "default": {}
        },
        "tokio_console": {
          "type": "boolean",
          "default": false
        }
      }
    },
    "EngineConfig": {
      "description": "Engine configuration for packet processing and buffering.",
      "type": "object",
      "properties": {
        "profile": {
          "description": "Optional tuning profile that provides sensible buffering defaults.\n\nExplicit values for `node_input_capacity` and/or `pin_distributor_capacity` take precedence.",
          "anyOf": [
            {
              "$ref": "#/$defs/EnginePerfProfile"
            },
            {
              "type": "null"
            }
          ],
          "default": null
        },
        "packet_batch_size": {
          "description": "Batch size for processing packets in nodes (default: 32)\nLower values = more responsive to control messages, higher values = better throughput",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 32
        },
        "node_input_capacity": {
          "description": "Buffer size for node input channels (default: 128 packets)\nHigher = more buffering/latency, lower = more backpressure/responsiveness\nFor low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame)",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "pin_distributor_capacity": {
          "description": "Buffer size between node output and pin distributor (default: 64 packets)\nFor low-latency streaming, consider 4-8 packets",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "oneshot": {
          "description": "Configuration for oneshot (HTTP batch) pipelines.",
          "$ref": "#/$defs/OneshotConfig",
          "default": {
            "packet_batch_size": 32,
            "media_channel_capacity": null,
            "io_channel_capacity": null
          }
        },
        "advanced": {
          "description": "Advanced buffer tuning for codec and container nodes.\nOnly modify if you understand the latency/throughput implications.",
          "$ref": "#/$defs/AdvancedBufferConfig",
          "default": {
            "codec_channel_capacity": null,
            "stream_channel_capacity": null,
            "demuxer_buffer_size": null,
            "moq_peer_channel_capacity": null
          }
        }
      }
    },
    "EnginePerfProfile": {
      "description": "Preset tuning profiles for the engine.",
      "oneOf": [
        {
          "description": "Low-latency real-time streaming (minimal buffering, more backpressure)",
          "type": "string",
          "const": "low-latency"
        },
        {
          "description": "Balanced defaults for general streaming and interactive pipelines",
          "type": "string",
          "const": "balanced"
        },
        {
          "description": "High-throughput / batch processing (more buffering, higher latency)",
          "type": "string",
          "const": "high-throughput"
        }
      ]
    },
    "OneshotConfig": {
      "description": "Oneshot pipeline configuration (HTTP batch processing).\n\nThese settings apply to stateless pipelines executed via the `/api/v1/process` endpoint.\nOneshot pipelines use larger buffers by default than dynamic sessions because they\ndon't require tight backpressure coordination.",
      "type": "object",
      "properties": {
        "packet_batch_size": {
          "description": "Batch size for processing packets in oneshot pipelines (default: 32)\nLower values = more responsive, higher values = better throughput",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 32
        },
        "media_channel_capacity": {
          "description": "Buffer size for media channels between nodes (default: 256 packets)\nOneshot uses larger buffers than dynamic for batch efficiency.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "io_channel_capacity": {
          "description": "Buffer size for I/O stream channels (default: 16)\nUsed for HTTP input/output streaming.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        }
      }
    },
    "AdvancedBufferConfig": {
      "description": "Advanced internal buffer configuration for power users.\n\nThese settings affect async/blocking handoff channels in codec and container nodes.\nMost users should not need to modify these values. Only adjust if you understand\nthe latency/throughput tradeoffs and have specific performance requirements.\n\nAll values are in packets (not bytes). The actual memory footprint depends on packet size.",
      "type": "object",
      "properties": {
        "codec_channel_capacity": {
          "description": "Capacity for codec processing channels (opus, flac, mp3) (default: 32)\nUsed for async/blocking handoff in codec nodes.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "stream_channel_capacity": {
          "description": "Capacity for streaming reader channels (container demuxers) (default: 8)\nSmaller than codec channels because container frames may be larger.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "demuxer_buffer_size": {
          "description": "Duplex buffer size for ogg demuxer in bytes (default: 65536)",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "moq_peer_channel_capacity": {
          "description": "MoQ transport peer channel capacity (default: 100)\nUsed for network send/receive coordination in MoQ transport nodes.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        }
      }
    },
    "PluginConfig": {
      "description": "Plugin directory configuration.",
      "type": "object",
      "properties": {
        "directory": {
          "type": "string"
        },
        "allow_http_management": {
          "description": "Controls whether runtime plugin upload/delete is allowed via the public APIs.\n\nDefault is false to avoid accidental exposure when running without an auth layer.",
          "type": "boolean",
          "default": false
        },
        "marketplace_enabled": {
          "description": "Enables the plugin marketplace API and UI (default: false).",
          "type": "boolean",
          "default": false
        },
        "allow_native_marketplace": {
          "description": "Allows native plugins to be installed from a marketplace (default: false).\n\nNative plugins run in-process and are unsafe without full trust.",
          "type": "boolean",
          "default": false
        },
        "allow_model_urls": {
          "description": "Allow direct URL model downloads from manifests (default: false).",
          "type": "boolean",
          "default": false
        },
        "marketplace_require_registry_origin": {
          "description": "Require marketplace URLs to share origin with the registry (default: false).",
          "type": "boolean",
          "default": false
        },
        "marketplace_scheme_policy": {
          "description": "Scheme policy for marketplace URLs (default: https_only).",
          "$ref": "#/$defs/MarketplaceSchemePolicy",
          "default": "https_only"
        },
        "marketplace_host_policy": {
          "description": "Host policy for marketplace URLs (default: public_only).",
          "$ref": "#/$defs/MarketplaceHostPolicy",
          "default": "public_only"
        },
        "marketplace_resolve_hostnames": {
          "description": "Resolve hostnames for marketplace URLs and check resolved IPs (default: false).",
          "type": "boolean",
          "default": false
        },
        "marketplace_url_allowlist": {
          "description": "Allowed marketplace origins (e.g., \"https://example.com\", \"https://example.com:*\").",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "trusted_pubkeys": {
          "description": "Minisign public keys (contents of `.pub` files) trusted for marketplace manifests.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "registries": {
          "description": "Registry index URLs (e.g., `https://example.com/index.json`).",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "models_dir": {
          "description": "Optional directory to store downloaded models (defaults to `models` when unset).",
          "type": [
            "string",
            "null"
          ],
          "default": null
        },
        "huggingface_token": {
          "description": "Optional Hugging Face token for gated model downloads.",
          "type": [
            "string",
            "null"
          ],
          "default": null
        }
      },
      "required": [
        "directory"
      ]
    },
    "MarketplaceSchemePolicy": {
      "type": "string",
      "enum": [
        "https_only",
        "allow_http"
      ]
    },
    "MarketplaceHostPolicy": {
      "type": "string",
      "enum": [
        "public_only",
        "allow_private"
      ]
    },
    "ResourceConfig": {
      "description": "Resource management configuration for ML models and shared resources.",
      "type": "object",
      "properties": {
        "keep_models_loaded": {
          "description": "Keep loaded resources (models) in memory until explicit unload (default: true).\nWhen false, resources may be evicted based on LRU policy if max_memory_mb is set.",
          "type": "boolean",
          "default": true
        },
        "max_memory_mb": {
          "description": "Optional memory limit in megabytes for cached resources (models).\nWhen set, least-recently-used resources will be evicted to stay under the limit.\nOnly applies when keep_models_loaded is false.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0
        },
        "prewarm": {
          "description": "Pre-warming configuration for reducing first-use latency",
          "$ref": "#/$defs/PrewarmConfig",
          "default": {
            "enabled": false,
            "plugins": []
          }
        }
      }
    },
    "PrewarmConfig": {
      "description": "Configuration for pre-warming plugins at startup.",
      "type": "object",
      "properties": {
        "enabled": {
          "description": "Enable pre-warming (default: false)",
          "type": "boolean",
          "default": false
        },
        "plugins": {
          "description": "List of plugins to pre-warm with their parameters",
          "type": "array",
          "items": {
            "$ref": "#/$defs/PrewarmPluginConfig"
          },
          "default": []
        }
      }
    },
    "PrewarmPluginConfig": {
      "description": "Configuration for a single plugin to pre-warm at startup.",
      "type": "object",
      "properties": {
        "kind": {
          "description": "Plugin kind (e.g., \"plugin::native::kokoro\", \"plugin::native::whisper\")",
          "type": "string"
        },
        "params": {
          "description": "Parameters to use when creating the warmup instance\nThese should match the most common usage pattern",
          "default": null
        },
        "fallback_params": {
          "description": "Optional fallback parameters to try if the primary params fail\nUseful for GPU plugins that should fallback to CPU",
          "default": null
        }
      },
      "required": [
        "kind"
      ]
    },
    "PermissionsConfig": {
      "description": "Permission configuration section for skit.toml.",
      "type": "object",
      "properties": {
        "default_role": {
          "description": "Default role for requests without an authenticated role\n\nWhen built-in auth is disabled, this becomes the effective role for requests that are not\nassigned a role via a trusted role header or `SK_ROLE`.\n\nFor production deployments, prefer enabling built-in auth (`[auth].mode`) or running behind\nan authenticating reverse proxy that sets `[permissions].role_header`.",
          "type": "string",
          "default": "admin"
        },
        "role_header": {
          "description": "Optional trusted HTTP header used to select a role (e.g. \"x-role\" or \"x-streamkit-role\").\n\nIf unset, StreamKit ignores role headers entirely and uses `SK_ROLE`/`default_role`.\n\nSecurity note: Only enable this when running behind a trusted reverse proxy or\nauth layer that (a) authenticates the caller and (b) strips any incoming header\nwith the same name before setting it.",
          "type": [
            "string",
            "null"
          ],
          "default": null
        },
        "allow_insecure_no_auth": {
          "description": "Allow starting the server on a non-loopback address without built-in auth or a trusted role\nheader.\n\nThis only applies when built-in auth is disabled.\n\nThis is unsafe: all requests fall back to `SK_ROLE`/`default_role`. The server refuses to\nstart in this configuration unless this flag is set.",
          "type": "boolean",
          "default": false
        },
        "roles": {
          "description": "Map of role name -> permissions",
          "type": "object",
          "additionalProperties": {
            "$ref": "#/$defs/Permissions"
          },
          "default": {
            "user": {
              "create_sessions": true,
              "destroy_sessions": true,
              "list_sessions": true,
              "modify_sessions": true,
              "tune_nodes": true,
              "load_plugins": false,
              "delete_plugins": false,
              "list_nodes": true,
              "list_samples": true,
              "read_samples": true,
              "write_samples": true,
              "delete_samples": true,
              "allowed_samples": [
                "oneshot/*.yml",
                "oneshot/*.yaml",
                "dynamic/*.yml",
                "dynamic/*.yaml",
                "user/*.yml",
                "user/*.yaml"
              ],
              "allowed_nodes": [
                "audio::*",
                "video::*",
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
                "core::sink",
                "plugin::*"
              ],
              "allowed_plugins": [
                "plugin::*"
              ],
              "access_all_sessions": false,
              "upload_assets": true,
              "delete_assets": true,
              "allowed_assets": [
                "samples/audio/system/*",
                "samples/audio/user/*"
              ]
            },
            "viewer": {
              "create_sessions": false,
              "destroy_sessions": false,
              "list_sessions": true,
              "modify_sessions": false,
              "tune_nodes": false,
              "load_plugins": false,
              "delete_plugins": false,
              "list_nodes": true,
              "list_samples": true,
              "read_samples": true,
              "write_samples": false,
              "delete_samples": false,
              "allowed_samples": [
                "oneshot/*.yml",
                "oneshot/*.yaml",
                "dynamic/*.yml",
                "dynamic/*.yaml",
                "user/*.yml",
                "user/*.yaml"
              ],
              "allowed_nodes": [
                "*"
              ],
              "allowed_plugins": [
                "*"
              ],
              "access_all_sessions": false,
              "upload_assets": false,
              "delete_assets": false,
              "allowed_assets": [
                "samples/audio/system/*"
              ]
            },
            "admin": {
              "create_sessions": true,
              "destroy_sessions": true,
              "list_sessions": true,
              "modify_sessions": true,
              "tune_nodes": true,
              "load_plugins": true,
              "delete_plugins": true,
              "list_nodes": true,
              "list_samples": true,
              "read_samples": true,
              "write_samples": true,
              "delete_samples": true,
              "allowed_samples": [
                "*"
              ],
              "allowed_nodes": [
                "*"
              ],
              "allowed_plugins": [
                "*"
              ],
              "access_all_sessions": true,
              "upload_assets": true,
              "delete_assets": true,
              "allowed_assets": [
                "*"
              ]
            }
          }
        },
        "max_concurrent_sessions": {
          "description": "Maximum concurrent dynamic sessions (global limit, applies to all users)\nNone = unlimited",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0,
          "default": null
        },
        "max_concurrent_oneshots": {
          "description": "Maximum concurrent oneshot pipelines (global limit)\nNone = unlimited",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint",
          "minimum": 0,
          "default": null
        }
      }
    },
    "Permissions": {
      "description": "Represents a set of permissions granted to a role\n\nNote: We allow excessive bools here because permissions are inherently\nindependent boolean flags. Each field represents a distinct capability\nthat can be enabled or disabled. Converting to enums or state machines\nwould complicate the API without providing meaningful benefit.\nRole-based permissions for access control.",
      "type": "object",
      "properties": {
        "create_sessions": {
          "description": "Can create new sessions",
          "type": "boolean",
          "default": false
        },
        "destroy_sessions": {
          "description": "Can destroy sessions (their own or any depending on context)",
          "type": "boolean",
          "default": false
        },
        "list_sessions": {
          "description": "Can list sessions (their own or all depending on context)",
          "type": "boolean",
          "default": false
        },
        "modify_sessions": {
          "description": "Can modify running sessions (add/remove nodes)",
          "type": "boolean",
          "default": false
        },
        "tune_nodes": {
          "description": "Can tune parameters on running nodes",
          "type": "boolean",
          "default": false
        },
        "load_plugins": {
          "description": "Can upload and load plugins (WASM or native)",
          "type": "boolean",
          "default": false
        },
        "delete_plugins": {
          "description": "Can delete plugins",
          "type": "boolean",
          "default": false
        },
        "list_nodes": {
          "description": "Can view the list of available nodes",
          "type": "boolean",
          "default": false
        },
        "list_samples": {
          "description": "Can list sample pipelines",
          "type": "boolean",
          "default": false
        },
        "read_samples": {
          "description": "Can read sample pipeline YAML",
          "type": "boolean",
          "default": false
        },
        "write_samples": {
          "description": "Can save/update user pipelines in `[server].samples_dir/user`",
          "type": "boolean",
          "default": false
        },
        "delete_samples": {
          "description": "Can delete user pipelines in `[server].samples_dir/user`",
          "type": "boolean",
          "default": false
        },
        "allowed_samples": {
          "description": "Allowed sample pipeline paths (supports globs like \"oneshot/*.yml\").\n\nPaths are evaluated relative to `[server].samples_dir`.\nEmpty list means no samples are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "allowed_nodes": {
          "description": "Allowed node types (e.g., \"audio::gain\", \"transport::moq::*\")\nEmpty list means no nodes are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "allowed_plugins": {
          "description": "Allowed plugin node kinds (e.g., \"plugin::native::whisper\", \"plugin::wasm::gain\", \"plugin::*\")\nEmpty list means no plugins are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "access_all_sessions": {
          "description": "Can access any user's sessions (admin capability)",
          "type": "boolean",
          "default": false
        },
        "upload_assets": {
          "description": "Can upload audio assets",
          "type": "boolean",
          "default": false
        },
        "delete_assets": {
          "description": "Can delete audio assets (user assets only)",
          "type": "boolean",
          "default": false
        },
        "allowed_assets": {
          "description": "Allowed audio asset paths (supports globs like \"samples/audio/system/*.opus\")\nEmpty list means no assets are allowed (deny by default).\nUse `[\"*\"]` to allow everything.",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        }
      }
    },
    "ScriptConfig": {
      "description": "Configuration for the core::script node.",
      "type": "object",
      "properties": {
        "default_timeout_ms": {
          "description": "Default timeout for script execution per packet (in milliseconds)",
          "type": "integer",
          "format": "uint64",
          "minimum": 0,
          "default": 100
        },
        "default_memory_limit_mb": {
          "description": "Default memory limit for QuickJS runtime (in megabytes)",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 64
        },
        "global_fetch_allowlist": {
          "description": "Global fetch allowlist (empty = block all fetch() calls)\nApplies to all script nodes.\n\nSecurity note: there is no per-pipeline allowlist override; this prevents bypass via\nuser-provided pipelines.",
          "type": "array",
          "items": {
            "$ref": "#/$defs/AllowlistRule"
          },
          "default": []
        },
        "secrets": {
          "description": "Available secrets (name → environment variable mapping)\nEmpty map = no secrets available to any script node\nSecrets are loaded from environment variables at server startup\nand can be injected into HTTP headers via pipeline configuration",
          "type": "object",
          "additionalProperties": {
            "$ref": "#/$defs/SecretConfig"
          },
          "default": {}
        }
      }
    },
    "AllowlistRule": {
      "description": "URL allowlist rule for fetch() API in script nodes.",
      "type": "object",
      "properties": {
        "url": {
          "description": "URL pattern with wildcards (e.g., \"https://api.example.com/*\")",
          "type": "string"
        },
        "methods": {
          "description": "Allowed HTTP methods",
          "type": "array",
          "items": {
            "type": "string"
          }
        }
      },
      "required": [
        "url",
        "methods"
      ]
    },
    "SecretConfig": {
      "description": "Configuration for a single secret loaded from environment.",
      "type": "object",
      "properties": {
        "env": {
          "description": "Environment variable name containing the secret value",
          "type": "string"
        },
        "type": {
          "description": "Type of secret (for validation and formatting)",
          "$ref": "#/$defs/SecretType",
          "default": "string"
        },
        "allowed_fetch_urls": {
          "description": "Optional allowlist of URL patterns where this secret may be injected into `fetch()` headers.\n\nPatterns use the same format as `script.global_fetch_allowlist` entries:\n- `https://api.openai.com/*`\n- `https://api.openai.com/v1/chat/completions`\n\nEmpty = no additional restriction (backwards-compatible).",
          "type": "array",
          "items": {
            "type": "string"
          },
          "default": []
        },
        "description": {
          "description": "Optional description for documentation",
          "type": "string",
          "default": ""
        }
      },
      "required": [
        "env"
      ]
    },
    "SecretType": {
      "description": "Type of secret for validation and documentation.",
      "oneOf": [
        {
          "description": "URL (e.g., webhook URLs)",
          "type": "string",
          "const": "url"
        },
        {
          "description": "Bearer token",
          "type": "string",
          "const": "token"
        },
        {
          "description": "API key",
          "type": "string",
          "const": "apikey"
        },
        {
          "description": "Generic string",
          "type": "string",
          "const": "string"
        }
      ]
    },
    "CompositorServerConfig": {
      "description": "Server-level defaults for the video compositor node.\n\nThese limits apply to every compositor node created by the engine.\nIndividual nodes cannot exceed these values, even via `UpdateParams`.\n\n```toml\n[compositor]\nmax_canvas_dimension = 7680\nmax_font_size = 4096\nmax_text_length = 10000\n```",
      "type": "object",
      "properties": {
        "max_canvas_dimension": {
          "description": "Maximum allowed canvas dimension (width or height) in pixels.\nDefault: 7680 (8K UHD).",
          "type": "integer",
          "format": "uint32",
          "minimum": 0,
          "default": 7680
        },
        "max_font_size": {
          "description": "Maximum allowed font size for text overlays in pixels.\nDefault: 4096.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0,
          "default": 4096
        },
        "max_text_length": {
          "description": "Maximum allowed text overlay string length in bytes.\nDefault: 10000.",
          "type": "integer",
          "format": "uint",
          "minimum": 0,
          "default": 10000
        }
      }
    },
    "AuthConfig": {
      "description": "Authentication configuration for built-in JWT-based auth.",
      "type": "object",
      "properties": {
        "mode": {
          "description": "Authentication mode (auto, enabled, disabled)",
          "$ref": "#/$defs/AuthMode",
          "default": "auto"
        },
        "state_dir": {
          "description": "Directory for auth state (keys, tokens). Default: \".streamkit/auth\"",
          "type": "string",
          "default": ".streamkit/auth"
        },
        "cookie_name": {
          "description": "Cookie name for browser sessions. Default: \"skit_session\"",
          "type": "string",
          "default": "skit_session"
        },
        "api_default_ttl_secs": {
          "description": "Default TTL for API tokens in seconds. Default: 86400 (24 hours)",
          "type": "integer",
          "format": "uint64",
          "minimum": 0,
          "default": 86400
        },
        "api_max_ttl_secs": {
          "description": "Maximum TTL for API tokens in seconds. Default: 2592000 (30 days)",
          "type": "integer",
          "format": "uint64",
          "minimum": 0,
          "default": 2592000
        },
        "moq_default_ttl_secs": {
          "description": "Default TTL for MoQ tokens in seconds. Default: 3600 (1 hour)",
          "type": "integer",
          "format": "uint64",
          "minimum": 0,
          "default": 3600
        },
        "moq_max_ttl_secs": {
          "description": "Maximum TTL for MoQ tokens in seconds. Default: 86400 (1 day)",
          "type": "integer",
          "format": "uint64",
          "minimum": 0,
          "default": 86400
        }
      }
    },
    "AuthMode": {
      "description": "Authentication mode for the server.",
      "oneOf": [
        {
          "description": "Auto: disabled on loopback, enabled on non-loopback",
          "type": "string",
          "const": "auto"
        },
        {
          "description": "Always require authentication",
          "type": "string",
          "const": "enabled"
        },
        {
          "description": "Disable authentication entirely (NOT recommended for production)",
          "type": "string",
          "const": "disabled"
        }
      ]
    }
  }
}
```

</details>
