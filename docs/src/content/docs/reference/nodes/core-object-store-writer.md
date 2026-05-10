---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::object_store_writer"
description: "Streams binary data to S3-compatible object storage (AWS S3, GCS, Azure, MinIO, RustFS, etc.). Uses multipart upload for bounded memory usage. Credentials can be provided via config or environment variables. Set passthrough: true to forward packets downstream (required for oneshot pipelines)."
---

`kind`: `core::object_store_writer`

Streams binary data to S3-compatible object storage (AWS S3, GCS, Azure, MinIO, RustFS, etc.). Uses multipart upload for bounded memory usage. Credentials can be provided via config or environment variables. Set passthrough: true to forward packets downstream (required for oneshot pipelines).

## Categories
- `io`
- `object_store`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `access_key_id` | `null | string` | no | — | Access key ID.<br /><br />If omitted, the node falls back to `access_key_id_env`. |
| `access_key_id_env` | `null | string` | no | `null` | Environment variable name containing the access key ID.<br /><br />Read at node startup.  Takes precedence over `access_key_id`. |
| `bucket` | `string` | yes | — | Bucket name. |
| `chunk_size` | `integer (uint)` | no | `5242880` | Buffer size before flushing to the object store (default: 5 MiB).<br /><br />This controls the multipart upload part size.  S3 requires a minimum<br />part size of 5 MiB (except the last part).<br />min: `5242880` |
| `content_type` | `null | string` | no | `null` | Optional MIME content type for the uploaded object<br />(e.g. `audio/ogg`, `video/mp4`). |
| `endpoint` | `string` | yes | — | S3-compatible endpoint URL.<br /><br />Examples:<br />- AWS S3: `https://s3.amazonaws.com`<br />- MinIO / RustFS: `http://localhost:9000`<br />- Cloudflare R2: `https://<account>.r2.cloudflarestorage.com` |
| `key` | `string` | yes | — | Object key (path within the bucket). |
| `passthrough` | `boolean` | no | `false` | When `true`, the node forwards every incoming packet to its `"out"`<br />pin in addition to writing it to object storage.  This allows the<br />node to sit inline in a linear pipeline (required for oneshot mode<br />which does not support fan-out).<br /><br />Default: `false` (pure sink — no output pin). |
| `region` | `string` | no | `us-east-1` | AWS region (default: `us-east-1`).<br /><br />Most S3-compatible services accept any region string; set this to<br />match the bucket's actual region for AWS S3. |
| `secret_access_key` | `null | string` | no | — | Secret access key.<br /><br />If omitted, the node falls back to `secret_key_env`. |
| `secret_key_env` | `null | string` | no | `null` | Environment variable name containing the secret access key.<br /><br />Read at node startup.  Takes precedence over `secret_access_key`. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the object store write node.",
  "properties": {
    "access_key_id": {
      "description": "Access key ID.\n\nIf omitted, the node falls back to `access_key_id_env`.",
      "type": [
        "string",
        "null"
      ],
      "writeOnly": true
    },
    "access_key_id_env": {
      "default": null,
      "description": "Environment variable name containing the access key ID.\n\nRead at node startup.  Takes precedence over `access_key_id`.",
      "type": [
        "string",
        "null"
      ]
    },
    "bucket": {
      "description": "Bucket name.",
      "type": "string"
    },
    "chunk_size": {
      "default": 5242880,
      "description": "Buffer size before flushing to the object store (default: 5 MiB).\n\nThis controls the multipart upload part size.  S3 requires a minimum\npart size of 5 MiB (except the last part).",
      "format": "uint",
      "minimum": 5242880,
      "type": "integer"
    },
    "content_type": {
      "default": null,
      "description": "Optional MIME content type for the uploaded object\n(e.g. `audio/ogg`, `video/mp4`).",
      "type": [
        "string",
        "null"
      ]
    },
    "endpoint": {
      "description": "S3-compatible endpoint URL.\n\nExamples:\n- AWS S3: `https://s3.amazonaws.com`\n- MinIO / RustFS: `http://localhost:9000`\n- Cloudflare R2: `https://<account>.r2.cloudflarestorage.com`",
      "type": "string"
    },
    "key": {
      "description": "Object key (path within the bucket).",
      "type": "string"
    },
    "passthrough": {
      "default": false,
      "description": "When `true`, the node forwards every incoming packet to its `\"out\"`\npin in addition to writing it to object storage.  This allows the\nnode to sit inline in a linear pipeline (required for oneshot mode\nwhich does not support fan-out).\n\nDefault: `false` (pure sink — no output pin).",
      "type": "boolean"
    },
    "region": {
      "default": "us-east-1",
      "description": "AWS region (default: `us-east-1`).\n\nMost S3-compatible services accept any region string; set this to\nmatch the bucket's actual region for AWS S3.",
      "type": "string"
    },
    "secret_access_key": {
      "description": "Secret access key.\n\nIf omitted, the node falls back to `secret_key_env`.",
      "type": [
        "string",
        "null"
      ],
      "writeOnly": true
    },
    "secret_key_env": {
      "default": null,
      "description": "Environment variable name containing the secret access key.\n\nRead at node startup.  Takes precedence over `secret_access_key`.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "endpoint",
    "bucket",
    "key"
  ],
  "title": "ObjectStoreWriteConfig",
  "type": "object"
}
```

</details>
