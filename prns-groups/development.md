# Group development tools

These tools support fixed-membership native regression trials. Use
[authenticated enrollment](enrollment.md) for app groups and invitations. The
phone app uses identity-scoped stores and does not load these fixtures into a
named identity. Copying a primary-identity fixture into an app container cannot
provision a managed group.

## Fixed-membership fixtures

No public test identity or globally shared private group key is baked in. Obtain
the public Reticulum identity hash from each independently initialized native
endpoint, then generate one private fixture per endpoint:

```sh
cargo run --locked --manifest-path prns-groups/Cargo.toml -p prns-groups --bin dev-group-fixture -- NEW_OUTPUT_DIRECTORY "Development group" FIRST_IDENTITY_HEX SECOND_IDENTITY_HEX
```

Identity hashes are 32 hexadecimal characters, not LXMF destination hashes. The
directory must not already exist. The generator uses OS entropy, independently
enrolls 2–8 MLS members and derives the `prns.groups` destinations from their
supplied Reticulum identities. It emits `member-1.json`, `member-2.json`, etc. in
argument order, with restricted Unix permissions. Each fixture contains only
that endpoint's private MLS state and authenticated group/peer configuration.

The native fixture loader validates the supplied identity and atomically imports
into an empty namespace. Later opens verify the configuration and load current
ratchets; they never restore the initial snapshot over existing state. Corrupt
or mismatched fixtures fail closed. The headless peer below retains the fixture
for subsequent starts; independent test consumers supply it through the fixture
API and own their storage lifecycle.

Fixtures contain plaintext secrets. Transfer them privately, install each on
exactly one endpoint and do not commit or publish them. Cloning a fixture or
restoring it after deleting a used database can fork/reuse MLS ratchet state.
Generate a fresh group for a deliberate reset. Fixtures are enrollment artifacts,
not recovery backups. Retaining them also retains initial ratchet secrets; this
path does not establish at-rest forward secrecy or protected key retirement.

The fixed profile permits text up to 2048 UTF-8 bytes, 256 retained records and
a maximum 14-day delivery window. These are logical bounds in the fixture format,
not memory/file-size guarantees. Changing them requires explicit migration or
fresh enrollment. Record, output, receipt and backend object limits are separate:
a record-count ceiling does not guarantee every receipt pattern fits. Retire
eligible work through the runner/store APIs while preserving replay floors and
exact pending output.

## Headless development peer

The optional `dev-group-peer` executable is a native consumer of the same runner,
store lane and shared host APIs used by the app. Its `headless` feature adds TCP and
executable host ownership; it does not change the reusable
runner or embedded dependencies. Build from the repository root:

```sh
CARGO_INCREMENTAL=0 cargo build --locked --manifest-path prns-groups/Cargo.toml -p prns-groups --features headless --bin dev-group-peer
prns-groups/target/debug/dev-group-peer init /private/tmp/group-peer
```

`init` prints the public `identity` and `destination` as JSON. It generates and
persists a private identity using OS entropy only on the first invocation; later
invocations return the same public identity. Include this identity hash along with
the other native endpoints' identity hashes when running `dev-group-fixture` above. Copy
**only this peer's** fixture to `/private/tmp/group-peer/group-fixture.json`, then
start it:

```sh
prns-groups/target/debug/dev-group-peer run /private/tmp/group-peer --listen 0.0.0.0:4242
```

Connect the other test peers to this computer's reachable LAN address and port
4242. The listener enables Reticulum Transport, allowing connected members
to establish links through it as well as exchange stored group envelopes with it.
Use a private test network and allow the port through the host firewall. A second
headless endpoint can use `run ITS_ROOT --connect HOST:4242`. This is ordinary
Reticulum TCP traffic; the command interface remains on local stdin. No remote
command or fixture-download server is exposed.

The process emits JSON lines to stdout, diagnostics to stderr. A `ready` event
reports its public identity, destination and actual listening address (including
an OS-assigned port when listening on port 0). Enter one command per line:

```json
{"op":"status"}
{"op":"request_id"}
{"op":"send","request_id":"<copy the returned request_id>","text":"Hello from the headless peer"}
{"op":"history","before":null,"limit":50}
{"op":"stop"}
```

Use `request_id` to generate a fresh timestamped token from OS entropy for each
new send. Copy the returned hexadecimal `request_id` into the send command;
arbitrary random 32-byte strings are not valid request tokens. Keep that ID and the
exact text until an `accepted` response is reconciled. Retrying the same pair,
including after process restart, returns the original `record_id`; changing the
text under the same ID returns `idempotency_conflict`. Results have a 14-day retry
window; `retry_expired` means the old token must not be resubmitted as a fresh
send under a replacement ID without checking history. `storage_full` preserves
the original retry identity and requires capacity to become available.
Acceptance means durable
local storage, not delivery. History reports `received_count` and
`recipient_count`, never read status. IDs are hexadecimal; timestamps and feed positions are decimal strings to preserve
their exact values. History returns an opaque hexadecimal `position` for each
message and `next_before` for continuation. Pass that exact string as `before`
for the next page, even when the page contains no messages. The cursor binds the
group and retained membership incarnation; an old bare sequence is not supported.
`visited_heads` describes only this bounded page's sources; `current_incarnation`
reveals readmission during traversal.

Commands are capped at 16 KiB and history at 100 rows. Invalid commands return an
`error` event without shutting down the peer. `status` reports whether the sync
worker is running or unavailable; running does not confirm connectivity or
recipient delivery. Durable reads and send acceptance remain available after a
sync failure, which requires a process restart to resume synchronization. EOF and
`stop` both stop and join synchronization, stop the host, then fence and drain
accepted database work. Force termination is recovered from the durable database
on the next run; an ambiguous send still requires retry with the same request ID.

Keep `identity`, `group-fixture.json`, `application.redb` and `network/` together in
the private root. On Unix, the root and `network/` directories must have no
group/world permissions (normally 0700), and identity/fixture files must be private
regular files (normally 0600). Symlink roots, directories and private files are
refused; existing permissions are never silently changed. `init` creates new
roots with 0700 permissions, and `run` requires an existing identity and fixture
and never mints a replacement. The development fixture/key limitations described above apply.
The default caller poll interval is 15 seconds; the runner also applies its
interface-aware policy floors and bounded retry backoff. Development measurements
can set `--poll-ms 100..60000`. Fast polling is for controlled tests,
not a radio or battery policy.

The `headless_process` suite runs three independent executables over real TCP:
initial delivery, receipt convergence, offline catch-up through a remaining
member after the author exits, force termination and exact-ID retry after reopen.
It also checks stable initialization, wrong-identity fixture rejection, public or
symlinked private path rejection, and bounded invalid command recovery:

```sh
CARGO_INCREMENTAL=0 cargo test --locked --manifest-path prns-groups/Cargo.toml -p prns-groups --features headless --test headless_process
```

These process tests require permission to bind loopback TCP sockets. They do not
establish physical phone, LAN firewall or radio interoperability results.
