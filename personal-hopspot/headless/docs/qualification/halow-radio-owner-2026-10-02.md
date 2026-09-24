# Durable HaLoW radio owner qualification

The G4 and both Heltec HT-HD01-V2 boards passed protected radio activation and
automatic restoration of their operating vendor radio. Preparation preserved all
seven inspected vendor file hashes. Missing recovery protection and pending UCI
edits refused activation before writes. A deliberately conflicting RAM-only
binding edit during an applied trial was bypassed and cleared by recovery on all
three boards. Matching files alone could not finish recovery: the owner required
a new boot epoch, interface UP/carrier and successful Morse health.

[Structured evidence](halow-radio-owner-2026-10-02.json) records artifacts,
candidates, epochs, hashes and limits. The implementation is committed locally
as `61c7c365a`, following the separate filesystem extraction `fd8abf90b`.
Builds were made before the implementation commit and record their dirty source
base in the build metadata. This is development qualification, not a release or
electrical flash power-loss qualification.

## Owner and admission

The Linux manager now owns a separate device-wide journal at
`/etc/hopspot-radio`. Application slots, node identity and controller grants
remain separate. It stages private originals, modes and candidate files behind a
sealed recovery guard, uses alternating journals, and syncs mutation intent
before replacing vendor files. A failed publication poisons that writer until
reopened. Lost journals with a valid guard select restoration; damaged backups
refuse host mutation. A valid confirmation remains authoritative if old mutation
intent later becomes damaged.

`radio prepare` requires an explicit profile, recovery window and trial boot
allowance. `apply`, `confirm` and `rollback` require the current candidate's exact
revision and profile digest. Another pending or failed recovery blocks preparation.
The independent `hopspot-radio-recovery` procd service must be boot-enabled and
running with the matching manager root before activation. It starts before the
vendor network service, survives an application failure and never announces an
application endpoint or confirms a trial automatically.

The lease uses boot UUID and uptime. Each allowed trial reboot consumes persisted
allowance; ordinary checks cannot renew the lease or rewrite flash metadata.
Restoration has explicit phases for originals, reboot and operating-radio checks.
The reboot deadline is 60 seconds; post-reboot radio readiness has a separate
360-second deadline. Unavailable recovery remains nonterminal and blocks a new
trial. Confirmation remains a trusted operator decision after end-to-end health
checks through another gateway, as described in the
[deployment procedure](../halow-deployment.md).

## Hardware procedure and outcomes

The G4 led the initial trial/reboot test; both Heltecs followed. The explicit lab
profile remained US 924 MHz / 8 MHz, fixed MCS2, long guard, 18 dBm, open 802.11s,
no mesh data forwarding, no gate announcements and proactive HWMP rootmode 2.
The manager left normal Wi-Fi, Ethernet, DHCP and firewall outside its mutation
scope. Original vendor LED/button services were left running. These checks did
not measure throughput or power use.

| Board | Final candidate revision | Final journal revision | Restored boot UUID | Operating original radio |
| --- | ---: | ---: | --- | --- |
| ThinkNode G4 | 13 | 19 | `f804e8e4-2927-47e6-9246-9f3ccf3a0f8c` | AP, 924 MHz / 8 MHz |
| First Heltec | 10 | 16 | `f62f3a2d-2055-40dd-a79c-11f64fa84ba1` | Station with carrier, 924 MHz / 8 MHz |
| Second Heltec | 10 | 16 | `597768fd-a12f-471d-96c7-27bb96b4a718` | AP, 908 MHz / 8 MHz |

Each board passed shadow preparation with unchanged hashes, a stopped-service
refusal, real mesh activation, expiry-driven rollback, matching original files,
operating radio and late-confirmation refusal. The final fault additionally staged
`wireless.radio1.type=prns_pending_qualification` in ordinary UCI RAM state. The
owner refused this before activation, then successfully recovered when the same
fault was left staged after activation.

Source review and hardware checks corrected two UCI assumptions. `-t` changes the
save directory but retains ordinary delta search paths in
[libuci](https://github.com/openwrt/uci/blob/master/delta.c). Absolute-file loading
bypasses those deltas in its [file backend](https://github.com/openwrt/uci/blob/master/file.c).
Staging and recovery inspection now use absolute file paths. UCI's
[batch implementation](https://github.com/openwrt/uci/blob/master/cli.c) also does
not propagate every command failure. The owner therefore checks all intended
candidate values, removed credentials/network binding and exactly one long-guard
list member before publishing preparation. Hardware confirmed that assigning an
empty network value removes the option; the typed plan now expresses removal
explicitly.

Radio startup was asynchronous: an immediate query after successful activation
could report a missing device, followed by the expected mesh/rate settings. The
G4's scheduled trial reboot also temporarily removed the Mac's DHCP management
address. Both pinned IPv4 and scoped IPv6 management paths were investigated;
neither was continuously available throughout reboot. The original path returned
after operational restoration. The intermediate G4 trial boot UUID was not
captured during that outage; this report does not claim a boot-latency measurement
or continuous management availability. A timed-out second Heltec manager upload
was reconciled by its actual complete on-device hash before proceeding.

## Verification and artifacts

- `cargo test -p personal-hopspot-appliance --all-targets`: 31 library tests,
  three shipping CLI tests and three separate lab CLI tests passed on macOS arm64.
- `cargo clippy -p personal-hopspot-appliance --all-targets -- -D warnings`, package
  formatting and service shell syntax passed.
- The final 31-test static MIPS artifact passed on each physical board, using RAM
  directories and a fake vendor adapter. This exercises the actual target ISA,
  Linux filesystem/locking and owner fault model; it is not injected electrical
  corruption of the boards' flash.
- Fault tests cover every observed preparation, application, replacement-guard,
  confirmation and restoration publication step; stale candidates; missing
  protection; corrupt journals, intent and backups; repeated trials; reboot and
  readiness deadlines; boot allowance; and ordinary checks preserving inode
  metadata. Candidate tests reject partial projections, wrong rates, retained
  bridge bindings/credentials and duplicate guard options.
- The pinned MIPS builder produced separate shipping and lab artifacts. The
  shipping manager is 1,480,440 bytes, SHA-256
  `c42d50cc10235ff77e3b60cd9cd045bcde5c7e366be18e5233d2708cfbd5ad56`.
  The lab manager is 1,481,960 bytes, SHA-256
  `95b884c833be3c782701e77325caf3bf09f77cfc4200a63ccf78d5e0b479ceb9`.
- The G4 conflicting-edit trial used the immediately preceding lab artifact,
  SHA-256 `a7613465304103fdb6bd408d482eeb3faf78d41c7e5051d763ef5fb2003312d1`.
  The final change avoids redundant directory chmod during owner reopening; its
  inode-metadata regression passed in the final test artifact on all three boards.
  Both Heltec conflicting-edit trials used the final lab artifact.
- `python3 validation/run.py verify` passed. `./tools/prns verify` still reports
  the preexisting three misplaced `headless/scripts/network-lab` implementations;
  this qualification does not count that check as passing.

Private vendor backups and radio-state exports were retained locally outside
tracked source. Temporary managers, services, journals and RAM files were removed
after verification. Final vendor hashes, radio health and management connectivity
were rechecked; the Mac's Internet default route remained on Wi-Fi `en1` through
`192.168.4.1`.

No new PRNS over-air control/page campaign ran in this slice. The unchanged
headless application's [prior three-board mesh qualification](halow-mesh-boot-2026-10-02.md)
is separate evidence. The next gates are coordinated electrical power-loss tests,
manager/bootstrap upgrade handling, signed release enrollment and the guided
web/local-helper installer. The browser should present the manager's transaction
states and preserve target/key pinning, rather than owning another radio state
machine. Discovery, interrupted-upload reconciliation and locally named timeout
stages belong in that guided flow.
