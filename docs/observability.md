# Observability

`prnsd` owns Prns's host observability pipeline. It emits human or JSON events and can export bounded operation traces plus fixed, low-cardinality runtime metrics over OTLP.

## Run the local backend

The included local backend uses Grafana's pinned LGTM image: Grafana, Prometheus, Loki, Tempo, and an OpenTelemetry Collector in one disposable container. Its ports bind only to localhost.

Prerequisites are Docker with Compose and the repository's Rust toolchain. On macOS, start Docker Desktop, OrbStack, or Colima first; on Windows, start Docker Desktop.

```sh
cargo observability
```

This starts the pinned LGTM container, waits until it is healthy, prints the dashboard and OTLP endpoints, and exits. It does not start `prnsd`. Repeated runs reconcile the same container. `docker compose` and `docker-compose` are both supported.

Run a source-built daemon separately with the non-default OTLP feature and point it at the collector. The official cloud container already compiles this capability:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_METRIC_EXPORT_INTERVAL=5000 \
  cargo prnsd restart --detach --features otlp -- --log-format json
```

On Windows, set the same variables in PowerShell first:

```powershell
$env:OTEL_EXPORTER_OTLP_ENDPOINT = 'http://127.0.0.1:4318'
$env:OTEL_METRIC_EXPORT_INTERVAL = '5000'
cargo prnsd restart --detach --features otlp -- --log-format json
```

To select a Reticulum config directory, put daemon arguments after Cargo's `--` separator:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_METRIC_EXPORT_INTERVAL=5000 \
  cargo prnsd restart --detach --features otlp -- \
    --config "$HOME/.reticulum" --log-format json
```

(On Windows, set the `$env:` variables as in the PowerShell block above;
PowerShell's own `$HOME` works in the `--config` argument.)

Open [the Prns health dashboard](http://127.0.0.1:3000/d/prns-observability/prns-health). The preset view includes:

- daemon liveness
- five-minute failure breakdowns
- uptime, configured-versus-discovered interfaces, routes, links, shared clients, traffic, and sampled request latency
- inbound and outbound announces by source, origin, outcome, and interface kind
- announce holds, schedules, pacer depth, lossless egress backpressure, lane occupancy, and terminal egress failures
- warnings, errors, and recent structured events

Metrics and traces travel over OTLP. Structured events remain in the daemon log. The local collector reads `prnsd.jsonl` from the shared per-user state directory for the Loki panels.

Remove the backend when it is no longer needed:

```sh
cargo observability down
```

This will not stop `prnsd`; do that independently if needed.

## Operate `prnsd`

`cargo prnsd` and a built `prnsd` executable manage the same per-user daemon on macOS, Linux, and Windows. The Cargo command builds before a new start; the released executable is self-contained. Repeated invocations immediately attach to the existing process without rebuilding or starting a second daemon. Ctrl-C detaches while leaving the daemon running.

| Command | Behavior |
| --- | --- |
| `cargo prnsd` or `cargo prnsd start` | Start if needed, show the Prns header, and attach to the log |
| `cargo prnsd --detach` | Start if needed and return to the shell without attaching |
| `cargo prnsd logs` | Show the Prns header and attach to the existing daemon log |
| `cargo prnsd restart [BUILD OPTIONS] [-- PRNSD OPTIONS]` | Build first, then gracefully replace the daemon and attach |
| `cargo prnsd stop` | Show recent logs, then gracefully stop while streaming the shutdown logs; repeated stops are harmless |
| `cargo prnsd build [BUILD OPTIONS]` | Produce a locked, OTLP-capable release artifact and print its absolute path |

`cargo prnsd status` is the prefixless RNS network-status utility. It is a one-shot client of the
running shared instance, not a lifecycle command. The full one-shot suite is documented in
[`docs/prnsd-utilities.md`](prnsd-utilities.md).

Use `restart` to replace a running daemon with different build options, daemon arguments, or environment. The stop is graceful and performs the daemon's final persistence flush.

```sh
cargo prnsd restart --debug -- --config "$HOME/.reticulum"
```

The release profile remains the default. Select a development build with `--debug`, or another Cargo profile with `--profile`. Build options belong before `--`; daemon options belong after it. `cargo prnsd -- --help` and `cargo prnsd -- --version` remain one-shot daemon commands and do not start the managed session.

The managed state directory is `${XDG_STATE_HOME:-~/.local/state}/prnsd` on Linux, `~/Library/Application Support/prnsd` on macOS, and `%LOCALAPPDATA%\prnsd` on Windows. Human output is stored in `prnsd.log`. Selecting `--log-format json` stores the same stable event names and fields in `prnsd.jsonl` for the local Grafana stack. `RUST_LOG` is captured when the daemon starts or restarts. `PRNSD_STATE_DIR` selects an isolated state directory when needed.

Useful `RUST_LOG` filters include `warn`, `info`, and `debug` for broad settings. You can also adjust individual subsystems by target: `prns.runtime` carries the runtime's link, announce, and command events; `prns.interface` carries interface configuration and connection-attempt spans; `prnsd` carries the daemon's own lifecycle, persistence, and discovery events; `prns_interfaces_tokio` carries per-connection transport diagnostics. For example: `RUST_LOG=info,prns.runtime=debug,prnsd=debug`.

Invalid filters fail startup. Levels mean: `error` is a hard failure requiring attention even when the daemon can continue, `warn` is failed or degraded side work, `info` is a sparse lifecycle transition, and `debug` carries frequent activity or correlation fields.

```sh
RUST_LOG=debug,prns.runtime=info cargo prnsd restart
```

```powershell
$env:RUST_LOG = 'debug,prns.runtime=info'; cargo prnsd restart
```

The built executable accepts the same lifecycle commands without Cargo. `prnsd run` is the explicit foreground mode for terminals, and `prnsd run --service --config DIR` registers that process for native service operation. The portable managed session survives terminal exit, but does not start at login or boot and does not restart after a crash. The [deployment guide](deploy-prnsd.md#systemd) provides a hardened systemd unit.

The default `tray` feature publishes the Prns mark after daemon readiness on macOS, Windows, and
Linux. Its concise status menu reports the live logical-interface state and provides these
platform-owned actions:

- **Open Prns Terminal** opens an attached managed log session. Ctrl-C detaches without stopping
  the daemon. Foreground `prnsd run` sessions identify themselves and leave this action disabled
  because they do not own a managed log.
- **Show Network Status** opens `prnsd status` against the effective configuration.
- **Manage Interfaces…** opens the guided `prnsd interfaces` editor against that same configuration.
- **Open Configuration Folder** reveals the effective directory, including an explicit `--config`
  override.
- **Stop prnsd** enters the same graceful persistence and background-task shutdown path as
  `prnsd stop`.

Terminal actions target the exact running executable and carry an isolated `PRNSD_STATE_DIR`
through to the child session; they do not depend on the user's shell `PATH`. Tray setup remains
best-effort: headless sessions continue normally and emit an informational `tray_unavailable` when
the platform tray service is absent, while failures preparing local tray actions remain warnings.
Service-oriented builds can omit it with
`--no-default-features --features tokio-host,observability`.

OTLP metrics and traces are an explicit build capability. The official cloud container and canonical `cargo prnsd build` artifact include it; custom source and native service builds must select the `otlp` feature. Export starts only when an endpoint is configured for that signal and `OTEL_SDK_DISABLED` is not `true`.

The exporter uses OTLP/HTTP protobuf. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` can replace the common endpoint per signal. `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_TRACES_SAMPLER`, and `OTEL_SDK_DISABLED` are also honored.

If several `prnsd` processes publish to one backend, give each a stable `service.instance.id` through `OTEL_RESOURCE_ATTRIBUTES`.

Production traces default to parent-based 10% sampling. Remote trace export queues at most 2,048 spans, sends at most 512 per batch, and uses five-second network and shutdown bounds. Runtime state is sampled every five seconds, while `OTEL_METRIC_EXPORT_INTERVAL` controls how often the SDK exports those observations.

### Announce egress pressure and loss

Announce pacing treats a full outbound lane as temporary backpressure. The pacer retains the original queued frame and retries it without charging announce bandwidth until the lane admits it. The `prns.announces.backpressure` and `prns.interface.announces.backpressure` counters separate the lifecycle into `deferred` (once when an announce first encounters a full lane), `retry` (each later failed attempt), and `recovered` (eventual admission). These are pressure signals and do not contribute to the dashboard's announce-failure or health-degradation totals.

`prns.announces.pacer_deferred_depth` and `prns.announces.pacer_oldest_deferred_age_ms` show current retained work node-wide. Their interface views are `prns.interface.announces.queue_depth{queue="pacer_deferred"}` and `prns.interface.announces.oldest_deferred_age_ms`. Physical outbound capacity is exported as `prns.egress.lane.capacity` and `prns.egress.lane.occupancy`, labeled with both the physical lane and its logical interface. A `lane_full` outcome is a terminal failure only for a pacerless announce; retained pacer work is represented exclusively by the backpressure lifecycle.

Bounded shedding remains a terminal outcome for the affected interface attempt, but it is not a node fault. The dashboard's top-level summaries present pacer rejection, priority eviction, and pacerless lane-full shedding as blue egress-pressure signals and keep them out of Operational state and Hard signals. Missing lanes, IFAC rejection, and a pacer entry surviving to its 24-hour expiry remain hard failures. The detailed terminal-egress and backpressure time series use categorical colors so interfaces remain visually distinct as their number grows; their legends retain the origin and outcome or event meaning. The per-interface egress and backpressure counters identify the logical interface responsible for pressure instead of attributing an aggregate node-wide failure.

Queue or lane depth should not be raised merely because `LaneFull` occurred. Sustained physical-lane occupancy together with growing deferred depth, oldest age, and retry rates is the evidence for a capacity change. A low-occupancy lane with deferrals instead points toward scheduling or fairness, which this iteration measures but does not reserve capacity to solve.

Structured events remain on stderr for journald, Grafana Alloy, Vector, Fluent Bit, or another log collector.

## Why the observability layers are separate

| Layer | Responsibility | Why separate |
| --- | --- | --- |
| `log` | Portable diagnostics from Embassy, platform backends, FFI, and lower-level interfaces | Works across `no_std` and host boundaries without a tracing subscriber |
| `tracing` | Structured Tokio events and bounded operation spans | Provides fields, context, filtering, JSON output, and sampled OTLP traces |
| `runtime-metrics` | Exact cumulative counters, gauges, and snapshots | Remains unsampled and exporter-independent |

The default `prnsd/observability` feature provides human or JSON output and bridges portable `log` records into the tracing subscriber, giving the daemon one local output path rather than duplicate streams. The independent default `prnsd/tray` feature provides the system-tray integration described above. The non-default `prnsd/otlp` crate feature additionally enables runtime metrics and OTLP metric and trace export; official cloud images select it at build time. Logs remain on stderr.

There is no span per packet, frame, crypto operation, or resource segment. Spans cover bounded calls such as requests, sends, links, resources, persistence, and individual interface connection attempts.

With an `otlp` build that has no OTLP endpoint configured, no provider or reporter task starts. Without the feature, the daemon's OTLP dependencies and runtime counters are not compiled. The top-level `personal-rns` `tracing` and `runtime-metrics` features select the Tokio host lane and stay out of Embassy builds; embedded firmware can use portable `log` diagnostics or omit all three layers entirely.

Prns's structured events and spans record sizes and operational identifiers, not payload bodies, private keys, or secrets. Production retention and access policy should still treat debug output accordingly.

### Persistence notifications

The lower-level host worker injects `Journaled::PersistenceFlushed` and `Journaled::PersistenceFlushFailed` into the ordered engine journal because those notifications must retain their position relative to engine work even though the engine performs no storage I/O. Recipe-managed restoration and terminal persistence notifications still travel through the normal manifold and application event path and its panic boundary.

For `prnsd`, a runtime persistence failure is a degraded durability signal rather than a routing shutdown signal. The worker reports the failure and continues retrying while the live node retains its in-memory state. Startup remains fail-closed when required storage or the stable transport identity cannot be established, and a failed final shutdown flush remains a nonzero shutdown result.
