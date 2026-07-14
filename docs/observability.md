# Observability

Prns keeps observability compile-time optional and separates the data plane from the reporting backend. The engine does not depend on a collector, async exporter, global logger, or tracing subscriber. Hosts choose the signals and the destination.

## Feature boundaries

| Feature | Scope | Runtime cost when disabled |
| --- | --- | --- |
| `log` | Existing host and interface diagnostics through the `log` facade | The facade dependency and diagnostic argument evaluation are absent |
| `tracing` | Tokio operation spans and structured runtime/interface events | Instrument attributes and event emission are not compiled |
| `runtime-metrics` | Cumulative engine, egress, and crypto-pool counters exposed as snapshots | Counter storage and updates added by this feature are not compiled |
| `prnsd/observability` | Human or JSON stderr subscriber plus a `log`-to-`tracing` bridge | Enabled in the daemon's default build |
| `prnsd/otlp` | Batched OTLP/HTTP trace export | Not in the daemon's default build |

`tracing` and `runtime-metrics` are Tokio-host features. They do not enter the Embassy or `no_std` graph. Embedded firmware can enable `log` explicitly when its board installs a logger; otherwise all three signals remain out of the image.

## Signal policy

Prns does not create a span for each packet, frame, crypto operation, resource segment, or loop iteration. Spans cover bounded application operations such as a request, send, link establishment, resource transfer, or persistence flush. Packet-hot paths use cumulative counters only when `runtime-metrics` is enabled.

Structured events use stable `event` names and low-cardinality fields:

- `error` means the operation cannot continue without intervention.
- `warn` means a failed operation, dropped capability, or security-relevant anomaly.
- `info` means sparse lifecycle, role, configuration, and readiness transitions.
- `debug` carries frequent activity and correlation identifiers.

Payload bytes, keys, and secrets are never recorded. Structured runtime lifecycle events keep cryptographic and correlation identifiers at `debug`; the corresponding `info` event describes the transition without them. Backend warnings may include the failing operating-system endpoint or interface needed for diagnosis. Operators should still treat debug telemetry as sensitive operational data.

## Daemon output

The daemon writes diagnostics to stderr. Human output is the default:

```sh
cargo run --manifest-path prnsd/Cargo.toml -- --config /path/to/reticulum
```

JSON Lines output is selected explicitly and suppresses the human splash:

```sh
cargo run --manifest-path prnsd/Cargo.toml -- --log-format json
```

`RUST_LOG` controls the local subscriber. Invalid filters fail daemon startup instead of silently widening output. The built-in filter is:

```text
warn,prnsd=info,prns.runtime=info,prns.interface=info,prns_runtime=info,prns_interfaces_tokio=info,prns_ffi=info,personal_rns=info
```

Examples:

```sh
RUST_LOG=info cargo run --manifest-path prnsd/Cargo.toml -- --log-format json
RUST_LOG=warn,prns.runtime=debug cargo run --manifest-path prnsd/Cargo.toml
```

JSON stderr can be collected by Grafana Alloy, Promtail, Vector, Fluent Bit, journald, or another log pipeline without parsing the human formatter.

## OTLP traces

Remote export requires the non-default `otlp` build feature and an endpoint. Compiling the feature alone does not start an exporter thread.

```sh
cargo build --release --manifest-path prnsd/Cargo.toml --features otlp
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
  ./prnsd/target/release/prnsd
```

The exporter uses OTLP/HTTP protobuf and the standard OpenTelemetry environment variables, including signal-specific endpoints, headers, service name, resource attributes, and sampler controls. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` can be used instead of the general endpoint. `OTEL_SDK_DISABLED=true` disables remote export even when an endpoint is present.

The default root sampler is parent-based 10%. Set `OTEL_TRACES_SAMPLER` and, where required, `OTEL_TRACES_SAMPLER_ARG` to override it. The batch processor is bounded to 2,048 queued spans and 512 spans per batch, with a five-second schedule and network timeout. Shutdown also has a five-second bound.

Only trace spans are exported by this feature. Local structured events continue through stderr. Prns does not inject trace context into Reticulum wire packets.

## Runtime metrics

Enable `runtime-metrics` on `personal-rns` and request a reactor-serialized cumulative snapshot from the Tokio handle:

```rust
if let Some(snapshot) = handle.metrics_snapshot().await {
    let accepted = snapshot.egress.enqueued_frames;
    let dropped = snapshot.egress.full_lane_drops + snapshot.egress.missing_lane_drops;
    let ignored = snapshot.engine.ignored_packets.total();
}
```

The daemon's default `observability` build does not enable these counters. A host opts into `runtime-metrics` only when it consumes or exports snapshots. The snapshot contains:

- engine packet and command totals plus a fixed set of ignore-reason counters;
- egress enqueue, full-lane drop, and missing-lane drop totals;
- crypto submitted/completed jobs, current and maximum queue depth, backpressure deferrals, and outstanding packet verdicts when a pool is active;
- the engine instant at which the reactor serialized the snapshot.

Counters are process-lifetime, monotonic, and saturating. Ignore reasons are a fixed enum rather than strings or labels, so an adapter cannot accidentally create unbounded metric cardinality. The snapshot API is backend-neutral: a host may expose it to Prometheus, OpenTelemetry metrics, a local status surface, or a test harness without putting an exporter in the engine.

## Verification

The important feature combinations can be checked directly:

```sh
cargo check -p prns-runtime --no-default-features --features embassy-host
cargo test -p prns-runtime --features tracing,runtime-metrics,log
cargo check --manifest-path prnsd/Cargo.toml
cargo test --manifest-path prnsd/Cargo.toml --all-features
```

Performance comparisons should use release builds and measure three separate configurations: all observability features off, local structured tracing enabled with the default filter, and OTLP enabled at the intended sampling ratio. Packet throughput and tail latency should be compared before changing span placement or enabling a signal by default.
