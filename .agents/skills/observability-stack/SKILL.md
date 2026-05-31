---
name: observability-stack
description: >-
  Spin up StreamKit's local observability stack (skit + Prometheus + Grafana,
  optional speech gateway) and validate the Grafana dashboards end-to-end. Use
  when testing metrics/dashboards, debugging empty dashboard panels, or
  reproducing the speech-gateway monitoring setup locally.
license: MPL-2.0
---

# Observability stack (local)

`samples/observability/` is a `docker compose` stack that runs skit + Prometheus
+ Grafana (and an optional speech gateway), auto-provisioning both bundled
dashboards. Use it to validate metrics and dashboards without any cloud setup.

## Run it

```bash
cd samples/observability
docker compose up -d
./generate-traffic.sh                 # direct-to-skit TTS+STT
# optional gateway row:
docker compose --profile gateway up -d --build
./generate-traffic.sh --gateway
```

Grafana: <http://localhost:3000> (anonymous admin). Prometheus:
<http://localhost:9090>. skit: <http://localhost:4545>.

## How metrics flow

- **skit → Prometheus via OTLP push.** Prometheus runs with
  `--web.enable-otlp-receiver`; skit's `SK_TELEMETRY__OTLP_ENDPOINT` points at
  `…/api/v1/otlp/v1/metrics`. There is **no scrape job** for skit.
- **gateway → Prometheus via scrape** of the gateway's `/metrics`.

## Validate dashboards (don't just eyeball)

OTLP renames dotted metrics and appends unit suffixes, so verify the metric
names/labels the panels query actually exist before trusting a panel:

```bash
# list all metric names Prometheus knows about
curl -s localhost:9090/api/v1/label/__name__/values | jq -r '.data[]' | sort
# run a panel's exact PromQL and count series (0 == panel will be "No data")
curl -s --data-urlencode 'query=<promql>' localhost:9090/api/v1/query \
  | jq '.data.result | length'
# inspect a metric's labels
curl -s 'localhost:9090/api/v1/series?match[]=<metric>' | jq
```

Key name/label facts:

- Plugin metrics: `plugin_call_duration_seconds_*` (unit suffix present),
  `plugin_calls_total`; labels `plugin_kind`, `op`.
- `oneshot_pipeline_duration_*` has **no** `_seconds` suffix (no unit set);
  labels `status`, and `service` only when an `X-StreamKit-Service` header is
  forwarded by a service-label-aware skit.
- Gateway: `gateway_requests_total{endpoint,code}`,
  `gateway_request_duration_seconds`, `gateway_rejected_total{reason}` (only
  appears after a 413/415/502 actually occurs).

## Expected "No data" (not bugs)

- Plugin failure panels (`plugin_errors_total` etc.) — counters don't exist
  until a failure happens.
- Oneshot "by Service" panels — empty unless the skit build emits the `service`
  label.
- Video / MoQ / codec panels — only populate when you run those pipelines.

## Gotchas (most-common causes of empty dashboards)

- **`latest-demo` is stale.** Pin a versioned `-demo` tag; `latest-demo` can
  predate metrics like `plugin.call.duration`, leaving the Plugins row empty.
- **Demo-image plugin layout.** `-demo` images ship bare `.so` files but the
  loader wants `plugins/native/<id>/` bundles; `skit/entrypoint.sh` reassembles
  them. Symptom: "no plugins found" / "node kind not found in registry".
- **Model-name mismatch.** A pipeline's `model_path` must exist in the image's
  `models/`. The stack's `pipelines/` use the names the `-demo` image ships.
- **Grafana datasource input.** Committed dashboards use `${DS_PROMETHEUS}`;
  the `dashboard-prep` step rewrites it to the provisioned uid. In compose
  command strings, escape it as `$${DS_PROMETHEUS}` so compose doesn't
  interpolate it.
- **Local auth.** skit needs `SK_AUTH__MODE=disabled` +
  `SK_PERMISSIONS__ALLOW_INSECURE_NO_AUTH=true` to start unauthenticated on a
  non-loopback bind. Local only.
