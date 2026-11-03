# Saved connection profiles

Enable `prns-host-runtime/connections` to use one private installation-wide profile
store independently of identities, groups, an app, or a running host. The default
host feature graph does not include its persistence dependencies. Private-file
support follows `prns-storage` (currently Unix); unsupported platforms fail
explicitly.

`ProfileOwner::create_new(path, initial_profiles, options)` creates only a new file.
The caller supplies initial presets and an existing private parent directory.
`open_existing` validates the exact persisted schema/capacity and never inserts a
preset, creates missing schema, repairs malformed input, or migrates old formats.
The profile store owns its versioned encoding; configuration is serialized directly
from the canonical `prns-host::InterfaceConfig` declarations. This profile format
supports TCP clients and one LAN discovery configuration. Bluetooth/platform
preparation and arbitrary other interface kinds remain separate host capabilities.

Use `ProfileOwner::client()` for `snapshot` and `replace`. A replacement supplies
the exact store-incarnation/revision guard and profiles with stable caller-created
`ProfileId`s. Validation is bounded and does not resolve DNS or open sockets. A
no-op preserves the revision. Success means the immediate durable commit finished;
a storage error may follow durable success, so read the same owner before deciding
what to do. Accepted saves survive a lost waiter, including while owner shutdown
drains its queue. `Limits` bounds profile count and encoded storage; queue capacity
and redb cache are separate construction budgets.

`owner.bind(&host_session)` returns one `HostBinding`. It claims its actual host
attachments and uses that host's existing connection/retry machinery. The profile
worker polls one retained attachment operation at a time without an ambient Tokio
runtime or another executor. `BindingClient` is the borrowed snapshot/retry API;
its lifetime never postpones its owner's cleanup. Saved and applied revisions are
separate. Applied means every enabled profile is attached and all obsolete
attachments have actually retired; it does not mean a peer is connected or a
message delivered. Use ordinary host inspection for physical connections and
traffic counters. Removed profiles remain visible as retiring until confirmed.

Retry is fenced by the binding generation and exact saved guard. Its success means
that the retry was accepted, not completed. A newer committed disable prevents
reattachment after an earlier retirement completes. Failed attachment/retirement
has no automatic outer timer loop; normal TCP reconnect remains host-owned, and
explicit retry or a new saved revision can request reconciliation again. A partial
attachment whose rollback failed retains the exact claimed interface in Retiring
state instead of being mistaken for a no-effect failure.

Shutdown has two phases. `binding.fence()` immediately denies new retries and new
attachment admission while preserving interfaces needed by protocol cleanup.
After those protocols drain, `binding.close()` fences and starts retained actual
retirement. Its future and `closed()` observe that cleanup; dropping either waiter
does not cancel it. A failed attempt leaves Closing plus its typed failure, and an
explicit subsequent close can retry. Dropping the binding initiates the same
cleanup. A confirmed independent host stop also settles it; a command's Stopped
error alone is insufficient. The profile store remains editable while unbound.
Only `ProfileOwner::close` closes the storage owner, after accepted saves and its
binding drain. Construction and finalization never stop a supplied host.

## Owner checks

From the repository root:

```sh
python3 validation/run.py run --suite host-native-connections --suite host-native-connections-clippy
```

The tests include real loopback TCP across two native host generations, storage
CAS/reopen and future-format preservation, cancelled accepted saves, injected
precommit/lost-confirmation errors, two-phase fencing, and blocked provider cleanup
with responsive snapshots and late disable. Mobile permission/discovery behavior
still requires the existing platform/device qualification.
