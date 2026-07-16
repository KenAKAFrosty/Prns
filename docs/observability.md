# Observability

`prnsd` owns Prns's host observability pipeline. It emits human or JSON events and can export bounded operation traces plus fixed, low-cardinality runtime metrics over OTLP.

## Run the local backend

The included local backend uses Grafana's pinned LGTM image: Grafana, Prometheus, Loki, Tempo, and an OpenTelemetry Collector in one disposable container. Its ports bind only to localhost.

Prerequisites are Docker with Compose and the repository's Rust toolchain. On macOS, start Docker Desktop, OrbStack, or Colima first.

```sh
cargo observability
```

This starts the pinned LGTM container, waits until it is healthy, prints the dashboard and OTLP endpoints, and exits. It does not start `prnsd`. Repeated runs reconcile the same container. `docker compose` and `docker-compose` are both supported.

Run the daemon separately with the non-default OTLP feature and point it at the collector:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_METRIC_EXPORT_INTERVAL=5000 \
  cargo prnsd --detach --features otlp -- --log-format json
```

To select a Reticulum config directory, put daemon arguments after Cargo's `--` separator:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_METRIC_EXPORT_INTERVAL=5000 \
  cargo prnsd --detach --features otlp -- \
    --config "$HOME/.reticulum" --log-format json
```

Open [the Prns health dashboard](http://127.0.0.1:3000/d/prns-observability/prns-health). The preset view includes:

- daemon liveness
- five-minute failure breakdowns
- uptime, interfaces, routes, links, shared clients, traffic, and sampled request latency
- inbound and outbound announces by source, origin, outcome, and interface kind
- announce holds, schedules, pacer pressure, and egress failures
- warnings, errors, and recent structured events

Metrics and traces travel over OTLP. Structured events remain in the daemon log. The repository launcher writes JSON output directly to `prnsd/observability/data/prnsd.jsonl`, where the local collector can read it for the Loki panels.

Remove the backend when it is no longer needed:

```sh
cargo observability down
```

This will not stop `prnsd`; do that independently if needed.

## Operate `prnsd`

`cargo prnsd` manages one repository-local daemon on macOS and Linux. The first invocation builds it in release mode, starts it in the background, and attaches to its log. Repeated invocations attach to that same process without rebuilding or starting a second daemon. Ctrl-C detaches while leaving the daemon running.

| Command | Behavior |
| --- | --- |
| `cargo prnsd` or `cargo prnsd start` | Start if needed, show the Prns header, and attach to the log |
| `cargo prnsd --detach` | Start if needed and return to the shell without attaching |
| `cargo prnsd status` | Report whether the managed daemon is running, including its PID and log path |
| `cargo prnsd logs` | Show the Prns header and attach to the existing daemon log |
| `cargo prnsd restart [BUILD OPTIONS] [-- PRNSD OPTIONS]` | Build first, then gracefully replace the daemon and attach |
| `cargo prnsd stop` | Show recent logs, then gracefully stop while streaming the shutdown logs; repeated stops are harmless |

Use `restart` to replace a running daemon with different build options, daemon arguments, or environment. The stop is graceful and performs the daemon's final persistence flush.

```sh
cargo prnsd restart --debug -- --config "$HOME/.reticulum"
```

The release profile remains the default. Select a development build with `--debug`, or another Cargo profile with `--profile`. Build options belong before `--`; daemon options belong after it. `cargo prnsd -- --help` and `--version` remain one-shot daemon commands and do not start the service.

Human output is stored at `prnsd/.run/prnsd.log`. Selecting `--log-format json` stores the same stable event names and fields at `prnsd/observability/data/prnsd.jsonl` for the local Grafana stack. `RUST_LOG` is captured when the service starts or restarts.

Useful `RUST_LOG` filters include `warn`, `info`, and `debug` for broad settings. You can also adjust individual types of messages with `prns.runtime=debug,prns.interface=debug`, etc.

Invalid filters fail startup. Levels mean: `error` is a hard failure requiring attention even when the daemon can continue, `warn` is failed or degraded side work, `info` is a sparse lifecycle transition, and `debug` carries frequent activity or correlation fields.

```sh
RUST_LOG=debug,prns.runtime=info cargo prnsd restart
```

This repository command manages an interactive development service. Deployments that must start at login or boot should run the built `prnsd` binary under launchd, systemd, or another host supervisor.

OTLP metrics and traces are a non-default build feature. Export starts only when an endpoint is configured for that signal and `OTEL_SDK_DISABLED` is not `true`.

The exporter uses OTLP/HTTP protobuf. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` can replace the common endpoint per signal. `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_TRACES_SAMPLER`, and `OTEL_SDK_DISABLED` are also honored.

If several `prnsd` processes publish to one backend, give each a stable `service.instance.id` through `OTEL_RESOURCE_ATTRIBUTES`.

Production traces default to parent-based 10% sampling. Remote trace export queues at most 2,048 spans, sends at most 512 per batch, and uses five-second network and shutdown bounds. Runtime state is sampled every five seconds, while `OTEL_METRIC_EXPORT_INTERVAL` controls how often the SDK exports those observations.

Structured events remain on stderr for journald, Grafana Alloy, Vector, Fluent Bit, or another log collector.

## Why the observability layers are separate

| Layer | Responsibility | Why separate |
| --- | --- | --- |
| `log` | Portable diagnostics from Embassy, platform backends, FFI, and lower-level interfaces | Works across `no_std` and host boundaries without a tracing subscriber |
| `tracing` | Structured Tokio events and bounded operation spans | Provides fields, context, filtering, JSON output, and sampled OTLP traces |
| `runtime-metrics` | Exact cumulative counters, gauges, and snapshots | Remains unsampled and exporter-independent |

The default `prnsd/observability` feature provides human or JSON output and bridges portable `log` records into the tracing subscriber, giving the daemon one local output path rather than duplicate streams. The non-default `prnsd/otlp` feature additionally enables runtime metrics and OTLP metric and trace export. Logs remain on stderr.

There is no span per packet, frame, crypto operation, or resource segment. Spans cover bounded calls such as requests, sends, links, resources, persistence, and individual interface connection attempts.

With an `otlp` build that has no OTLP endpoint configured, no provider or reporter task starts. Without the feature, the daemon's OTLP dependencies and runtime counters are not compiled. The top-level `personal-rns` `tracing` and `runtime-metrics` features select the Tokio host lane and stay out of Embassy builds; embedded firmware can use portable `log` diagnostics or omit all three layers entirely.

Prns's structured events and spans record sizes and operational identifiers, not payload bodies, private keys, or secrets. Production retention and access policy should still treat debug output accordingly.
