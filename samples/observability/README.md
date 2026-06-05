<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Local observability stack

A `docker compose` stack that runs **skit + Prometheus + Grafana** (and an
optional **speech gateway**) so you can see StreamKit's metrics on the bundled
Grafana dashboards locally — no cloud, no manual import.

## Quick start

```bash
cd samples/observability
docker compose up -d            # skit + Prometheus + Grafana
./generate-traffic.sh           # drive ~20 TTS + STT requests through skit
```

Then open Grafana at <http://localhost:3000> (anonymous admin, no login). Two
dashboards are auto-provisioned:

- **StreamKit Performance Dashboard** — the repo's main dashboard
  ([`samples/grafana-dashboard.json`](../grafana-dashboard.json)), including the
  **Plugins / ML inference** row.
- **StreamKit Speech Gateway Dashboard** — the gateway/oneshot dashboard
  ([`examples/speech-gateway/grafana-dashboard.json`](../../examples/speech-gateway/grafana-dashboard.json)).

| Service    | URL                     |
| ---------- | ----------------------- |
| Grafana    | <http://localhost:3000> |
| Prometheus | <http://localhost:9090> |
| skit API   | <http://localhost:4545> |
| gateway    | <http://localhost:8080> (gateway overlay only) |

## How metrics get to Prometheus

Two different paths, both visible on the dashboards:

- **skit → Prometheus (OTLP push).** skit exports OTLP metrics to Prometheus'
  native OTLP receiver, which is enabled with `--web.enable-otlp-receiver`.
  Configured via `SK_TELEMETRY__OTLP_ENDPOINT` pointing at
  `http://prometheus:9090/api/v1/otlp/v1/metrics`. This feeds the HTTP, engine,
  oneshot, and **plugin** metrics.
- **gateway → Prometheus (scrape).** The speech gateway exposes a classic
  `/metrics` endpoint that Prometheus scrapes. That scrape lives in the gateway
  overlay (`prometheus.gateway.yml`, wired up by `docker-compose.gateway.yml`)
  rather than the base `prometheus.yml`, so the default stack has no
  perpetually-DOWN target. This feeds the **Speech Gateway** row.

## Speech Gateway row

The gateway lives in a compose overlay (`docker-compose.gateway.yml`) that adds
the service and points Prometheus at the config that scrapes its `/metrics`:

```bash
docker compose -f docker-compose.yml -f docker-compose.gateway.yml up -d --build
./generate-traffic.sh --gateway   # route traffic through the gateway
```

`--build` is only needed the first time, or after you change the gateway
sources under `examples/speech-gateway/`. To just bring the stack back up, drop
it:

```bash
docker compose -f docker-compose.yml -f docker-compose.gateway.yml up -d
```

Notes:

- The gateway's `/metrics` endpoint and the `gateway_*` metrics require the
  metrics-instrumented gateway. The Speech Gateway dashboard row stays empty
  until those metrics are present and the gateway has served traffic.
- The gateway's default STT pipeline targets a Whisper model that must exist on
  the skit it talks to. The bundled `-demo` image ships `ggml-tiny-q5_1.bin`; if
  the gateway points at a different model, STT through the gateway will fail
  while TTS still works. The direct-to-skit traffic path (the default
  `generate-traffic.sh`) avoids this by shipping its own pipelines under
  `pipelines/`.

## Known gotchas

These are the sharp edges worth knowing when wiring this up yourself:

- **Pin a versioned `-demo` tag.** `latest-demo` can lag behind released
  versions and predate metrics like `plugin.call.duration`, which leaves the
  Plugins / ML inference row empty. This stack pins `v0.5.0-demo`.
- **Demo image plugin layout.** Current `-demo` images ship native plugins as
  bare `.so` files under `plugins/native/`, but the loader expects directory
  bundles (`plugins/native/<id>/` with a `plugin.yml` + the `.so`). `skit serve`
  otherwise logs "no plugins found" and pipelines fail with "node kind not
  found". `skit/entrypoint.sh` reassembles the expected layout at startup from
  the in-repo manifests (mounted at `/repo-manifests`).
- **Model names must match.** Pipelines reference model files by path; the file
  must actually be present in the image/`models/` dir. The pipelines under
  `pipelines/` use the model names the `-demo` image actually ships.
- **Per-service oneshot panels (`by Service`) need a newer skit.** The oneshot
  pipelines declare `attributes: { service: tts|stt }`, which a service-label-
  aware skit turns into a bounded `service` metric label (operator opts in via
  `[server.metrics.attributes.service]` in `skit.toml`). The pinned
  `v0.5.0-demo` image predates this, so the dashboards' `by Service` panels stay
  "No data" until a newer `-demo` image is published — everything else
  populates. See the [observability guide](../../docs/src/content/docs/guides/observability.md)
  for the attribute mechanism.
- **Local auth override.** skit refuses to start unauthenticated on a
  non-loopback bind unless you opt in. This stack sets
  `SK_AUTH__MODE=disabled` + `SK_PERMISSIONS__ALLOW_INSECURE_NO_AUTH=true`, and
  to keep that safe every published port is bound to `127.0.0.1` so the
  unauthenticated skit and anonymous-admin Grafana stay reachable only from the
  host. **Local testing only** — never do this on an exposed instance.
- **Grafana dashboard datasource.** The committed dashboards use a
  `${DS_PROMETHEUS}` datasource input. The `dashboard-prep` step rewrites it to
  the provisioned datasource uid so the dashboards load without a manual import.

## Cleanup

```bash
docker compose -f docker-compose.yml -f docker-compose.gateway.yml down -v
```
