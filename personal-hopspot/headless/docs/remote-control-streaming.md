# Remote Control interface watch

The interface watch is a separately granted Remote Control capability. A target
advertises `WatchInterfaces` only when the service configures it and the runtime
installs its snapshot producer. Tokio installs that producer; portable and Embassy
assembly keep the capability absent. An authorized controller chooses a stream ID
and registers its reader before sending the request. The authenticated response must echo that ID;
admission failures remain silent.

The event frame is version 1 and exactly 14 bytes: version (1), kind (1),
sequence (4, big-endian), interface ID (8). `InterfaceChanged` and
`PeersChanged` invalidate corresponding paged snapshots. `ResyncRequired`
invalidates all snapshots. `Heartbeat` keeps idle links observable. The last
two kinds have a zero interface ID. A sequence gap requires a full refetch, and
each new subscription begins with a resync. Events contain no fabricated radio
readings or partial counters. The public Tokio reader parses frames and reports
sequence gaps as a typed error, so controller code can refetch snapshots. Reads
preserve partial frames across cancellation. Stream IDs belong to one reader per
link: duplicate registration fails locally before sending an admission request.
Use a fresh stream ID or reconnect rather than reusing an existing ID.

The Tokio producer compares stable interface state every 500 ms and coalesces
changes within the interval into one `ResyncRequired`. Byte counters, rate counters,
and changing radio measurements are excluded from that comparison. It sends a heartbeat every five seconds.
The node admits at most eight watches, bounds each reader to 32 chunks, and
stops a watch when its grant is revoked. Pending admissions are reserved before
waiting for a response lane, so revocation also cancels a watch waiting to start.
Canceled workers retain capacity until they finish. A blocked write times out
after two seconds; cleanup gets another two seconds to send EOF, then closes the
control link if it cannot finish. An owned watch driver runs alongside request handling; a blocked request operation cannot prevent watch timers from progressing. Node shutdown drops the futures synchronously. A panicking status callback closes its control link and releases the watch lease.
The runtime cancels watches on link closure, and reconnect starts a new
subscription. Dropping a reader alone does not unsubscribe; close its control
link to stop the subscription. Reader overflow and link loss have distinct typed
errors.

The [deterministic Remote Control campaign](../../../validation/simulation/measurements/remote-control.md) covers real-node admission, inventories, stream sequencing, packet faults and lifecycle checks under manual time. Further qualification needs an end-to-end multi-controller run on the G4 and
Heltec, including a peer join, a slow reader, and link closure. The producer
currently uses full resync invalidations; precise `InterfaceChanged` and
`PeersChanged` events can replace them when a direct change source is available.

## Host verification

The review added regressions for canceled partial reads, duplicate reader ownership,
revocation while waiting for a response lane, cancellation before stream start,
old-reservation cleanup after replacement, and blocked-writer cleanup. These run
on the development host; they do not qualify radio performance on the devices.

```sh
cargo test -p prns-core --lib remote_control
cargo test -p prns-runtime --lib
cargo test --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib
cargo test --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib
cargo test --manifest-path personal-hopspot/headless/Cargo.toml
cargo check -p prns-core -p prns-runtime --no-default-features
cargo clippy --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --tests -- -D warnings
cargo clippy --manifest-path personal-hopspot/headless/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path personal-hopspot/headless/Cargo.toml
```
