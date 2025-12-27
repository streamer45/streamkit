---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Script Node Guide
description: Execute custom JavaScript code for API integration, webhooks, and text processing
---

The script node (`core::script`) executes user-provided JavaScript code within your StreamKit pipelines. It provides a lightweight, sandboxed JavaScript runtime (QuickJS) for:

- **API Integration**: Call external REST APIs with the fetch() function
- **Webhooks**: Send notifications to Slack, Discord, or custom endpoints
- **Text Processing**: Transform, filter, or enrich text and transcription data
- **Dynamic Routing**: Add metadata for conditional routing downstream

## Quick Start

```yaml
mode: dynamic
steps:
  - kind: core::script
    params:
      script: |
        function process(packet) {
          if (packet.type === 'Text') {
            return {
              type: 'Text',
              data: packet.data.toUpperCase()
            };
          }
          return packet;
        }
```

Every script must define a `process(packet)` function that:
- Receives a packet object
- Returns a packet (modified or unchanged), or `null` to drop it

## Key Features

- ES2020 JavaScript with async/await support
- Fetch API with URL allowlist security
- Console API (log, error, warn)
- Telemetry API for UI timelines and spans
- Smart packet marshalling (metadata-only for audio/binary)
- Timeout protection (default: 100ms per packet)
- Memory limits (default: 64MB)
- Pass-through error handling (no pipeline breakage)

## Telemetry API

When running in a dynamic session, the script node exposes a global `telemetry` object for emitting timeline events (visible in the web UI and streamed over the WebSocket API as `nodetelemetry`):

- `telemetry.emit(event_type, data?)` → `true|false`
- `telemetry.startSpan(event_type, data?)` → `span_id` (string)
- `telemetry.endSpan(span_id, data?)` → `true|false`

`startSpan()` immediately emits an `${event_type}.start` event. `endSpan()` emits the final `${event_type}` event and adds `latency_ms` computed host-side.

Example:

```js
const turnId = `turn-${Date.now()}`;
const spanId = telemetry.startSpan('llm.request', { turn_id: turnId, model: 'gpt-4.1' });

const resp = await fetch('https://api.example.com/llm', { method: 'POST', body: '...' });

telemetry.endSpan(spanId, { turn_id: turnId, status: 'success', output_chars: resp.length });
```

## Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `script` | string | **(required\*)** | JavaScript code (must define `process(packet)`) |
| `script_path` | string | — | Path to a JavaScript file to load as the script (mutually exclusive with `script`) |
| `timeout_ms` | number | `100` | Per-packet execution timeout in milliseconds |
| `memory_limit_mb` | number | `64` | QuickJS heap size limit in megabytes |
| `headers` | array | `[]` | Header mappings for fetch() calls (maps secrets to HTTP headers) |

\* One of `script` or `script_path` is required.

### Server Configuration

The script node's fetch() capabilities are controlled by server configuration in `skit.toml`:

```toml
[script]
# Default timeout for all script nodes
default_timeout_ms = 100

# Default memory limit
default_memory_limit_mb = 64

# Global fetch allowlist (empty = block all fetch calls)
[[script.global_fetch_allowlist]]
url = "https://api.example.com/*"
methods = ["GET", "POST"]

[[script.global_fetch_allowlist]]
url = "https://hooks.slack.com/services/*"
methods = ["POST"]

# Secrets loaded from environment variables
[script.secrets]
slack_webhook = { env = "SLACK_WEBHOOK_URL", type = "url" }
api_key = { env = "API_KEY", type = "token" }
```

### Operational Limits

- `fetch()` requests are allowed only when the parsed URL host/path matches the global allowlist.
- StreamKit limits concurrent in-flight `fetch()` calls globally to reduce DoS risk. Override with `SK_SCRIPT_FETCH_MAX_INFLIGHT` (default: 16).

## Packet Types

### Text Packet

```javascript
{
  type: "Text",
  data: "Hello world"
}
```

### Transcription Packet

```javascript
{
  type: "Transcription",
  data: {
    text: "Full transcription text",
    language: "en",
    segments: [
      {
        text: "Segment text",
        start_time_ms: 0,
        end_time_ms: 1000,
        confidence: 0.95
      }
    ]
  }
}
```

### Audio Packet (Metadata Only)

Audio samples are NOT exposed to JavaScript for performance reasons. Only metadata is available:

```javascript
{
  type: "Audio",
  metadata: {
    sample_rate: 48000,
    channels: 2,
    frames: 480,
    duration_ms: 10
  }
}
```

### Custom Packet (Example: VAD)

```javascript
{
  type: "Custom",
  type_id: "plugin::native::vad/vad-event@1",
  encoding: "json",
  data: {
    event_type: "speech_start", // or "speech_end"
    timestamp_ms: 5000,
    duration_ms: 2000 // only for "speech_end"
  },
  metadata: {
    timestamp_us: 5000000
  } // optional
}
```

## Examples

### Text Transformation

```yaml
steps:
  - kind: core::script
    params:
      script: |
        function process(packet) {
          if (packet.type === 'Text') {
            return {
              type: 'Text',
              data: packet.data.toUpperCase()
            };
          }
          return packet;
        }
```

### API Integration

```yaml
steps:
  - kind: core::script
    params:
      timeout_ms: 2000
      script: |
        function process(packet) {
          if (packet.type === 'Text') {
            try {
              // Note: fetch() returns the response body as a string (it is implemented as a blocking call).
              const body = fetch('https://api.example.com/translate?text=' + encodeURIComponent(packet.data));
              const data = JSON.parse(body);
              return { type: 'Text', data: data.translated };
            } catch (e) {
              console.error('Translation failed:', e);
              return packet;
            }
          }
          return packet;
        }
```

### Content Filtering

```yaml
steps:
  - kind: core::script
    params:
      script: |
        function process(packet) {
          if (packet.type === 'Text') {
            // Only pass through text containing questions
            if (!/\?/.test(packet.data)) {
              return null;  // Drop packet
            }
          }
          return packet;
        }
```

### Language Detection

```yaml
steps:
  - kind: core::script
    params:
      script: |
        function process(packet) {
          if (packet.type === 'Text') {
            let lang = 'en';
            if (/[\u4e00-\u9fff]/.test(packet.data)) {
              lang = 'zh';
            } else if (/[\u3040-\u309f\u30a0-\u30ff]/.test(packet.data)) {
              lang = 'ja';
            }

            packet.metadata = {
              language: lang,
              tts_voice: lang === 'zh' ? 'zh-CN-female' : 'en-US-neutral'
            };
          }
          return packet;
        }
```

### Audio Stream Monitoring

```yaml
steps:
  - kind: core::script
    params:
      timeout_ms: 50
      script: |
        let stats = { total: 0, dropped: 0 };

        function process(packet) {
          if (packet.type === 'Audio') {
            const meta = packet.metadata;
            const duration = (meta.frames / meta.sample_rate) * 1000;

            stats.total++;

            // Log every 100 packets
            if (stats.total % 100 === 0) {
              console.log('Stream stats:', JSON.stringify(stats));
            }

            // Drop very short frames (likely artifacts)
            if (duration < 5) {
              stats.dropped++;
              return null;
            }
          }
          return packet;
        }
```

## Console API

```javascript
console.log("Debug message");      // DEBUG level
console.warn("Warning message");   // WARN level
console.error("Error message");    // ERROR level
```

View logs by running with debug logging:
```bash
RUST_LOG=streamkit::script=debug just skit serve
```

## Security

### URL Allowlist

The fetch() API requires URLs to match patterns in the server's `global_fetch_allowlist`. This prevents:
- Data exfiltration
- SSRF attacks
- Uncontrolled external dependencies

**Best practices:**
1. Be specific with URL patterns
2. Only allow methods you need
3. Use HTTPS exclusively
4. Review allowlist regularly

### Secrets Management

Never hardcode API keys in scripts. Use the secrets system:

```toml
# In skit.toml
[script.secrets]
api_key = { env = "MY_API_KEY", type = "token" }
```

```yaml
# In pipeline
- kind: core::script
  params:
    headers:
      - secret: api_key
        header: Authorization
        template: "Bearer {}"
```

## Anti-Patterns

### Don't Process Audio Samples

Audio samples are intentionally NOT exposed to JavaScript:

```javascript
// WRONG - this doesn't work!
function process(packet) {
  if (packet.type === 'Audio') {
    packet.samples.forEach(s => s * 0.5);  // samples don't exist!
  }
}
```

Use native nodes like `audio::gain` for audio processing.

### Don't Do Heavy Computation

```javascript
// WRONG - will timeout!
function process(packet) {
  for (let i = 0; i < 10000000; i++) {
    Math.sqrt(i);
  }
}
```

Keep scripts under the timeout limit (default 100ms).

## Performance Tips

1. **Return early** for packet types you don't process
2. **Minimize fetch() calls** - cache results when possible
3. **Keep scripts simple** - complex logic increases execution time
4. **Profile with console.log()** - measure execution time
5. **Consider native nodes** for performance-critical processing

```javascript
function process(packet) {
  // Fast path for irrelevant packets
  if (packet.type !== 'Text') {
    return packet;
  }

  // Only process what you need
  // ...
  return packet;
}
```

## See Also

- [Node Reference: core::script](/reference/nodes/core-script/) - Parameter details and schema
- [Configuration Reference](/reference/configuration/) - Server configuration options
