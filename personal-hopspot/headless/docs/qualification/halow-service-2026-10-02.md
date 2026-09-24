# Persistent HaLoW application service qualification

The G4 and both Heltec HT-HD01-V2 boards booted from compressed application slots
under procd and retained their pinned node/control identities and controller
grants. Authenticated build/interface snapshots, verified-controller AppMessage
and the exact 3,542-byte node page passed before and after reboot and confirmed
restart. The G4 also exhausted a trial and automatically returned to its verified
previous application. This is a development qualification of application service
and storage behavior, using wired management and unchanged vendor radio settings.
It is not a public installer or an over-air mesh-boot qualification.

[Structured evidence](halow-service-2026-10-02.json) records exact executable
hashes, boot UUIDs, resource snapshots, configuration hashes and limits. Builds
used `505d4824ce0fa9c356237374cdff7886fba5cffe` plus the uncommitted cold-boot
and application-manager changes. Rust nightly-2026-06-02 / Zig 0.15.2 produced
static MIPS32r2 little-endian O32 soft-float executables. Private evidence remains
in `/private/tmp/prns-appliance-qualification-20261002`; private configuration and
state backups are retained in ignored, mode-0700
`scratch/halow-appliance-private-20261002`. No private identities, passwords or
fixture signing secret are checked in.

## Owners and behavior

`personal-hopspot/appliance` owns the signed application package, compressed
slots, activation journal and bounded trial. Vendor firmware, radio settings and
calibration remain separate. Private application state lives outside replaceable
slots. The launch config has no arbitrary command, automatic announce or
controller-enrollment arguments: restarting cannot re-grant a revoked controller.

The shipping manager pins the repository release key. A separate qualification
example accepts the temporary lab public key and is excluded from the shipping
bundle. Final shipping-manager rejection of the lab-signed package passed on the
G4. This exercised the verifier and trust boundary; no public release was signed.
Every package verifies publisher signature, board, bounded compressed/expanded
lengths and hashes, static ELF headers and soft-float ABI flags before use.

The inactive slot is completed before publishing a trial journal. File and
directory synchronization bracket publication. Alternating checksummed journals
select the highest valid revision; torn newest metadata can fall back. Both
corrupt or absent journals beside installed slots refuse startup. Write failure
requires reopening rather than guessing whether a rename became durable.
Concurrent writers are locked. A candidate has a nonzero generation and validated
digest; confirmation must match both. Every trial launch is recorded before exec.
Exhaustion selects the verified previous slot, or fails closed on first install.
Health confirmation remains a trusted operator action following real control/page
checks, rather than a PID or listener heuristic.

Headless readiness was also corrected: it previously waited five seconds for a
radio broadcast child and exited if Linux had not created that device. It now
announces application readiness after core/wired setup; interface inventory
reports radio availability separately. On the G4, `prns-absent` never existed,
but authenticated wired operations and the exact page passed beyond that old
deadline. Managed radio rebind/discovery remains covered by the
[preceding recovery qualification](halow-recovery-2026-10-02.md).

## Physical checks

Each board had private vendor/state backups and an isolated boot service under
`/etc/prns-appliance-qualification`. An independent recovery service started
before it at boot and could stop/disable the lab app after 300 seconds. Its actual
expiry was observed on the G4; the app was deliberately restarted for final
collection. Wired management and the host internet interface remained separate.

The service used explicit `respawn 60 2 3`, a 15-second termination allowance,
128 open files and no core dumps. The manager uses Unix exec, so procd owns the
actual application's PID and signals. No radio-device restart trigger was added.
This follows the inspected vendor procd interface and OpenWrt's
[service configuration implementation](https://raw.githubusercontent.com/openwrt/openwrt/openwrt-23.05/package/system/procd/files/procd.sh).

| Final snapshot | G4 | Heltec | Heltec 3 |
| --- | ---: | ---: | ---: |
| App RSS, KiB | 3,988 | 3,976 | 3,984 |
| Open descriptors | 13 | 13 | 13 |
| Installation `du`, KiB | 4,145 | 4,145 | 4,145 |
| Private state `du`, KiB | 6 | 6 | 6 |
| Overlay free with two slots, KiB | 1,712 | 1,688 | 1,648 |
| Overlay free after cleanup, KiB | 5,308 | 5,192 | 5,160 |

The app is 3,631,976 bytes, gzip 1,507,694. The previous slot is the
3,636,456-byte recovery review candidate. The service-qualification shipping
manager is 1,213,980 bytes; the physically used, separate lab manager is 1,215,948
bytes. All service snapshots'
`/proc/PID/exe` hashes matched the new app, and all installed manager hashes
matched the final lab executable. These are snapshots, not sustained memory or
filesystem-growth bounds. The overlay also compresses some files, so its free
space and `du` are different measurements.

Final review corrected malformed signature-text error classification and checked
the one-byte read-budget overrun without integer overflow. The resulting shipping
manager is 1,214,332 bytes and separate lab manager 1,216,364 bytes. All eleven
updated owner tests passed again on all three MIPS boards, including these new
edges. The exact reviewed manager also passed RAM-slot staging, execution with
retained grants/identities, authenticated control, exact page and confirmation
on each board; its production signer refusal passed on the G4. The persistent
reboots were not repeated after these input-edge changes. Structured evidence
keeps both sets of executable hashes rather than treating them as identical.

The G4 first booted its unconfirmed candidate, then two deliberate application
crashes consumed the remaining launch and selected the previous slot. The pinned
controller and exact page worked throughout recovery. Restaging, confirming the
exact new candidate, stale-confirmation rejection and deliberate excessive-reserve
refusal passed. The test did not fill flash to create ENOSPC. With a confirmed
slot, a backed-up identity was temporarily replaced by seven corrupt bytes. The
app refused it, and procd reported a four-crash loop and stopped. Restoring the
original identity restored authenticated control. The final reviewed manager
also passed a second real G4 reboot with the confirmed slot.

Both Heltecs booted unconfirmed candidates, then passed exact confirmation and
graceful stop/restart. All three logged shutdown flushes for routing state and
ratchets followed by `hopspot_stopped`. Confirmed restarts left both activation
journal hashes unchanged; launch accounting does not write flash on every
confirmed boot. The app's existing periodic retained-state policy remains intact.

## Failure diagnosis and verification limits

The private SSH harness lost the first Heltec's staging completion acknowledgement
after the transaction succeeded. Its persisted journal was read before continuing;
the stage command was not blindly replayed. The second Heltec's initial lab config
bound IPv4 while its management connection used IPv6 link-local. That config was
corrected to `[::]:4347`. A later boot check also mishandled leading blank lines
in Heltec SSH output. Validated UUID comparisons and authenticated post-boot
checks established both reboots; reliable reboot timing is unavailable for them.

The G4 checks took 161.707 and 171.122 seconds from scheduled reboot through
management reacquisition and health. macOS temporarily lost its Ethernet DHCP
lease. These figures include the host/management path and cannot be reported as
application startup latency. A guided installer needs an explicit management
address-family and recovery contract. None of these timeouts alone proves denial.

The registered `hopspot-appliance-recovery` suite passed on macOS arm64: eleven
library tests and two command tests, with the command tests repeated in the
separate example. Seventeen injected publication interruptions cover staging,
launch accounting, confirmation and rollback. Additional cases cover exhaustion,
corrupt/torn/missing journals, concurrent writers, stale confirmations, signed
wrong hashes/oversized expansion/hard-float ABI, low-space refusal and unchanged
state. All eleven library tests also passed on each real MIPS/Linux board using
embedded public fixtures and disposable directories. These are filesystem fault
tests, not simulated flash electronics or actual power cuts.

Headless all-feature tests and Clippy passed on macOS; the new Linux-only
subprocess boot test does not run there, and its behavior was checked with real
authenticated G4 traffic. Appliance all-target Clippy, five G4 bundle fixture
tests, formatting, diff checks and registry verification passed. Linux-target
Clippy was not run. `./tools/prns verify` still reports the three pre-existing
misplaced network-lab scripts; it does not pass.

All five vendor configuration hashes matched afterward, pending UCI changes were
empty and Morse health passed. Original radio roles/channels remained in place.
The isolated apps, boot/recovery services, slots and device-side test backups
were stopped/disabled/removed after validation. No firmware image was flashed.

Physical power-loss/interrupted-flash recovery, production release signing and
bootstrap-manager trust, manager upgrades/key rotation, persistent regional mesh
and rate setup, over-air boot/recovery, least privilege, log/state growth and
sustained PRNS throughput remain gates. Those should precede a public install
button; this slice makes guided application activation concrete without claiming
the whole appliance lifecycle is qualified.
