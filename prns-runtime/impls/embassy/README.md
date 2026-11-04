# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## Bound route-journal replay work

`EmbeddedCompactionPolicy::with_route_replay_compaction_threshold` enables idle
maintenance when retained route upserts reach the configured threshold and a
snapshot would at least halve the replay work. Generic policies keep it disabled;
`hopspot` requests it after 256 route upserts with a persisted 24-hour minimum
attempt interval. Maintenance uses the incremental journal transaction. It
preserves critical state and only resets the replay counter after a confirmed
replacement commit. Reopening still validates the existing journal; completed
compaction bounds subsequent replay without changing the journal format.

Compaction starts only when route and ratchet writes, landing records and another
compaction are idle. The persisted attempt interval applies after reboot and
failed attempts, so repeated startup cannot bypass the write budget. A failed
replacement preserves the retained predecessor needed for recovery.

Run the persistence checks from the repository root:

```sh
python3 validation/run.py run --suite embedded-persistence-recovery
```

The suite covers trigger policy, retained predecessors, cooldown, failed writes,
reboot, torn I/O and authorization arriving during maintenance. It validates
recovery behavior on the host; firmware restore time and flash wear depend on
the selected board, storage and workload and require measurements of that image.
