# Bounded generated runtime contracts

The first campaign generates all six orderings of response expiry, request
cancellation and partition/reconnect. Two profiles pair a 1 ms deadline with
one-byte echoes and a 50 ms deadline with 256-byte echoes. Four runtime pairings
and cyclic/seeded outer actor order produce 96 bounded two-node scenarios.
Every phase verifies an independent whole expected result and successful
bidirectional traffic. Payload contents change with the phase.

The existing baseline and generated campaign share the same pair owner,
operations, clock coordination and teardown checks. The baseline's retired
connection checks remain in place: captured connection identities from before a
partition cannot carry any later traffic. Each scenario retains the existing
4,096-entry trace limits, explicit poll budgets and three-advertisement-interval
discovery bound. There is no timeout increase, silent trace truncation or retry
of failed cases.

Manifest tests assert the complete ordered case list. Failure output identifies
the action sequence, profile, runtime orientation, outer schedule and current
phase. The recipe and outer actor order are reproducible; ordinary Tokio
internal fairness and OS entropy are still deliberately exercised. This is
generated semantic coverage, not seeded complete-byte replay or automatic
failure shrinking. Existing explicitly controlled wire-replay tests remain.

## Finding and production fix

The campaign intermittently failed its reconnect bound in Tokio-only cases.
Temporary diagnostics showed a close notification from an obsolete connection
arriving after its replacement had been admitted at the same identity/address.
The old handler treated that notification as closure of the current member,
removed the replacement and notified the shared connection policy. This could
repeat across advertisement cycles.

Tokio BLE closure messages now retain the originating status-cell identity.
The receiver checks both address and status ownership before touching the member,
policy or backend. Unknown and obsolete closures are ignored. The reusable
`TokioInterfaceStatus::same_instance` operation compares existing Arc ownership;
stable logical interface IDs and on-wire messages are unchanged.

There is no new allocation or generation counter per connection. A close
notification temporarily retains one existing status handle and adds a
pointer-sized field to its queued payload. Live peer storage is unchanged.
An old status cell remains alive while its notification is queued, so address
reuse by the allocator cannot alias a replacement.

Focused tests cover clone versus reconstructed status identity, current/obsolete
close matching at the same logical ID and address, address mismatch, and real
peer close emission. Five successive 96-case campaign runs passed after the fix.
Before the fix, repeated runs reproduced failures, with diagnostics confirming
obsolete notifications were being applied.

Embassy's current receive/send closure paths retire the member selected from
their exclusively borrowed slot array; they do not enqueue this Tokio closure
message. All Embassy-only and both mixed orientations remain in the campaign.
The defect is in asynchronous adapter ownership, not shared protocol policy;
no no-std policy change or firmware RAM increase is warranted by this finding.

## Verification

Cargo commands use `CARGO_INCREMENTAL=0` on macOS arm64:

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble contracts:: --quiet
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto-runtime --lib bluetooth_auto --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib status_instance_identity --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
cargo clippy --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto-runtime --all-targets -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-targets -- -D warnings
python3 validation/run.py run --suite virtual-device-simulation
cargo test --workspace --locked --quiet
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

This does not cover physical Bluetooth, persistent reboot, ISA execution,
arbitrary operation sequences or large-fleet performance.

The full shared-clock target passed 102 tests. The registered simulation suite
passed in 83.502 seconds, including compilation/build-lock waiting; this is not
a runtime benchmark. Owner tests, root workspace tests, all three clippy checks,
format/docs, registries and website tests passed.
