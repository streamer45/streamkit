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
# optional gateway row (overlay adds the service + its Prometheus scrape):
docker compose -f docker-compose.yml -f docker-compose.gateway.yml up -d --build
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
  labels `status`, and `service` only when the pipeline declares
  `attributes: { service: ... }` and a service-label-aware skit has the matching
  `[server.metrics.attributes.service]` policy enabled.
- Gateway: `gateway_requests_total{endpoint,code}`,
  `gateway_request_duration_seconds`, `gateway_rejected_total{reason}` (only
  appears after a 413/415/502 actually occurs).

## Expected "No data" (not bugs)

- Plugin failure panels (`plugin_errors_total` etc.) — counters don't exist
  until a failure happens.
- Oneshot "by Service" panels — empty unless the skit build emits the `service`
  label (pipeline `attributes` + enabled `[server.metrics.attributes.service]`
  policy; the pinned `v0.5.0-demo` predates this).
- Video / MoQ / codec panels — only populate when you run those pipelines.

## Gotchas

The canonical list of setup gotchas (stale `latest-demo`, demo-image plugin
layout, model-name matching, `by Service` panels, local-auth override,
`${DS_PROMETHEUS}` datasource) lives in
[`samples/observability/README.md`](../../../samples/observability/README.md#known-gotchas)
— keep it there to avoid drift. One detail specific to *editing* the stack: in
compose `command:` strings, escape the datasource input as `$${DS_PROMETHEUS}`
so compose doesn't interpolate it before `dashboard-prep` runs.
