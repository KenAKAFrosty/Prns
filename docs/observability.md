# Observability

Prns keeps observability at the host boundary. `prnsd` emits human or JSON events and can export bounded operation traces plus fixed, low-cardinality runtime metrics over OTLP. The packet engine never requires a collector and does not create per-packet spans.

## Try the complete pipeline

The included local demo uses Grafana's pinned LGTM image: Grafana, Prometheus, Loki, Tempo, and an OpenTelemetry Collector in one disposable container. Its ports bind only to localhost.

Prerequisites are Docker with Compose and the repository's Rust toolchain.

```sh
docker compose -f examples/observability/compose.yaml up -d --wait
./examples/observability/run-demo.sh
```

Open [the Prns health dashboard](http://127.0.0.1:3000/d/prns-observability/prns-health). The preset view includes:

- separate daemon liveness, operational state, hard-failure, degraded-path, and host-integrity signals;
- five-minute failure breakdowns plus operation, resource, link, rejected-ingress, and capacity histories;
- uptime, interfaces, routes, links, shared clients, traffic, and sampled request latency;
- a logical-interface table plus selector-driven traffic, announce-flow, and queue-pressure views;
- inbound and outbound announces by source, origin, outcome, and interface kind;
- announce holds, schedules, pacer pressure, and egress failures;
- warnings, errors, and recent structured events.

Read the first row literally. `Daemon live` only proves fresh snapshots. `Operational state` is `HEALTHY`, `DEGRADED`, `FAULT`, or `DOWN`; its yellow state covers offline or unavailable paths, non-routine route removal, and crypto backpressure, while red means a hard runtime failure occurred within five minutes. `Host faults` independently covers persistence, identity, configuration, interface-start, and shared-instance failures from structured logs. The breakdown directly beneath names every active signal. Unavailable-interface egress is skipped work, not packet loss.

The demo connects to a closed local TCP port every five seconds, so interface activity remains visible without external traffic. It uses an always-on trace sampler and a five-second metric export interval to make short sessions useful. The daemon's JSON stderr is also tailed into Loki by the demo collector.

Press Ctrl-C to stop `prnsd`, then remove the backend:

```sh
docker compose -f examples/observability/compose.yaml down
```

To observe a real node, including NomadNet or another shared client, point the runner at that node's Reticulum config directory:

```sh
PRNSD_CONFIG="$HOME/.reticulum" ./examples/observability/run-demo.sh
```

## Operate `prnsd`

Human stderr is the default. JSON Lines carries the same stable event names and fields:

```sh
cargo run --manifest-path prnsd/Cargo.toml -- --config "$HOME/.reticulum"
RUST_LOG=info cargo run --manifest-path prnsd/Cargo.toml -- --log-format json
```

`RUST_LOG` controls local output. Useful filters include `warn`, `info`, and `warn,prns.runtime=debug,prns.interface=debug`. Invalid filters fail startup. Levels mean: `error` cannot continue, `warn` failed or degraded, `info` is a sparse lifecycle transition, and `debug` carries frequent activity or correlation fields.

OTLP metrics and traces are a non-default build feature. Export starts only when an endpoint is present:

```sh
cargo build --release --manifest-path prnsd/Cargo.toml --features otlp
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_METRIC_EXPORT_INTERVAL=5000 \
  ./prnsd/target/release/prnsd --log-format json
```

The exporter uses OTLP/HTTP protobuf and standard OpenTelemetry variables. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` can replace the common endpoint per signal. `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_TRACES_SAMPLER`, and `OTEL_SDK_DISABLED` are also honored.

If several `prnsd` processes publish to one backend, give each a stable `service.instance.id` through `OTEL_RESOURCE_ATTRIBUTES`.

Production traces default to parent-based 10% sampling. Remote trace export queues at most 2,048 spans, sends at most 512 per batch, and uses five-second network and shutdown bounds. Runtime state is sampled every five seconds; `OTEL_METRIC_EXPORT_INTERVAL` controls how often the SDK exports those observations.

Structured events remain on stderr for journald, Grafana Alloy, Vector, Fluent Bit, or another log collector. The local demo performs that log collection for you. Prns does not propagate trace context in Reticulum wire packets.

## Read the metrics precisely

Inbound announce metrics classify an announce after engine validation as accepted, held, ignored, or dropped from a bounded hold structure. Sources are `network` and `shared_client`.

Outbound announce metrics classify origin as `local`, `shared_client`, or `relay`. `outcome="enqueued"` means the frame passed any applicable pacing and IFAC handling and was accepted by an outbound interface lane. It does not claim physical-medium delivery. Paced relays skip unavailable interfaces instead of filling their lanes and report `interface_unavailable` separately; node-originated announces retain bounded reconnect buffering. Lane-full, missing-lane, IFAC, and pacer-rejection outcomes remain failures.

Per-interface series use the configured interface name and its logical medium. Temporary fleet members such as accepted TCP connections, discovered Wi-Fi peers, and shared-instance clients roll up beneath their supervisor. Routes mean destinations learned through that logical interface; local links attach to one interface; transported links are counted on every logical interface they touch, while the node-level transported total remains the unique link count. Ingress announce outcomes and held/scheduled depth follow the receiving logical interface. Egress outcomes, bytes, and pacer depth follow the transmitting logical interface.

Request latency comes from sampled `prns.request` spans, so its panels describe the sampled operation population rather than every packet. Route, link, interface, traffic, announce-pressure, and egress panels use unsampled fixed counters or gauges; bounded crypto health metrics are exported for custom views as well.

Reliability counters observe the runtime's central journal boundary, before an awaiting caller or resource sink consumes the event. `prns_operations_total` splits every settled public operation by operation and exact outcome. `prns_resources_failures_total`, `prns_links_closures_total`, `prns_links_interface_mismatches_total`, and `prns_routes_removals_total` retain bounded cause or reason labels. Sender-requested resource cancellation, peer-requested link closure, ordinary route expiry, and routine ignored-packet reasons remain available for analysis but do not become top-level hard failures.

Metric attributes use fixed enums for outcome, source/origin, interface kind, direction, link kind, queue kind, and ignore reason. The only operator-provided metric attribute is the bounded set of configured logical interface names. Destination hashes, interface IDs, peer IDs, packet bytes, and other dynamic values are never metric labels.

## Cost contract

| Feature | Adds | When absent |
| --- | --- | --- |
| `log` | Host/interface diagnostics through `log` | Dependency and arguments are absent |
| `tracing` | Tokio operation spans and structured events | Instrumentation is not compiled |
| `runtime-metrics` | Fixed cumulative engine, egress, announce, crypto, and reliability counters | Counter storage and updates are absent |
| `prnsd/observability` | Human/JSON subscriber and log bridge | Enabled in the daemon default |
| `prnsd/otlp` | Runtime counters plus bounded OTLP metric and trace export | Non-default; no exporter exists |

There is no span per packet, frame, crypto operation, resource segment, or loop iteration. Spans cover bounded calls such as requests, sends, links, resources, persistence, and interface connection. Ordinary packets retain fixed-array metric updates. Announce decisions additionally update one feature-gated logical-interface counter from the runtime's bounded configured topology; announce origin is carried as bounded metadata rather than rediscovered by reparsing packets. Reliability counters update only when a journaled command, resource failure, link closure or mismatch, or route removal already crosses the host boundary. Interface gauges are folded only on the reporter's five-second cadence.

With an `otlp` build but no endpoint, no provider or reporter task starts. Without the feature, the OTLP dependencies and runtime counters are not compiled. `tracing` and `runtime-metrics` stay out of Embassy and `no_std` graphs; embedded firmware can omit all three signal paths entirely.

Payloads, keys, and secrets are never recorded. Debug events can still reveal operational identifiers, so production retention and access policy should treat them accordingly.

The local LGTM stack is for development and demonstrations, not a production observability deployment.
