# Persistent HaLoW mesh boot qualification

The G4 and both Heltec HT-HD01-V2 boards retained the explicit US 924 MHz / 8 MHz,
fixed MCS2, long-guard, 18 dBm open-mesh profile across real reboots. After new
boot UUIDs were verified, authenticated build/interface/config/peer inspection,
verified-controller AppMessage and exact 3,542-byte pages passed through other
live mesh gateways. A final run fetched all six directed board-to-board page
pairs. Application identities and grants were retained; no controller was
reenrolled and no automatic application announcement was added.
Each board's HaLoW parent and its three child interface IDs (two neighbors plus
broadcast) matched between the warm and final post-reboot snapshots.

[Structured evidence](halow-mesh-boot-2026-10-02.json) records exact artifacts,
boot epochs, profile settings, tests and restoration. This is a bounded desk
qualification with explicit controller-seeded discovery. It does not qualify
unattended cold discovery, electrical power cuts, field range or sustained
throughput. The application is unchanged from the preceding
[service qualification](halow-service-2026-10-02.md).

## The missing-path finding

Disabling both mesh data forwarding and proactive HWMP requests initially looked
sufficient: peers were established and group announcements arrived. After the
second Heltec reboot, however, its kernel mesh path table was empty. Ethernet
captures showed the G4 emitting directed PRNS path requests which did not appear
at the Heltec's packet ingress. Its app remained healthy over wired management,
the packet socket matched the actual device index, and Morse health passed.
Explicit page announcements and a radio-only down/up did not restore directed
traffic under that policy.

Setting `mesh_hwmp_rootmode=2` on all three populated direct kernel paths and
restored authenticated control and the exact page without restarting the app.
The qualified profile therefore requires the typed `ProactiveRequests` path
policy. `mesh_fwding=0` and `mesh_gate_announcements=0` remain unchanged. The
inspected original vendor root modes were zero; this is a measured qualification
choice, not a claimed vendor default.

Linux implements proactive PREQ path selection through
[HWMP management action frames](https://raw.githubusercontent.com/torvalds/linux/v5.15/net/mac80211/mesh_hwmp.c).
That radio control traffic is separate from a Reticulum application announce.
The desk observation identifies the missing-path boundary and a working policy;
it does not reveal the closed radio firmware's exact failure mechanism. Management
airtime and behavior in a larger physical mesh still need measurement.

A first corrected-profile Heltec check also exhausted its 240-second discovery
probe budget, although wired control and the kernel paths were healthy when
inspected afterward. Both alternate gateways subsequently worked. A repeat reboot
passed the explicit controller workflow after verifying its new boot UUID. This
transient is retained in the evidence rather than treated as an authorization
failure or proof of automatic cold-discovery reliability.

## Radio preparation and ownership

The application manager now offers a read-only `radio-plan --profile PATH`.
The [typed profile](../../../appliance/openwrt/radio-profile.example.json) requires
the named radio/interface/device binding, regional channel, rate preset, path
setup and mesh ID. It rejects malformed identifiers, unsupported choices,
unknown profile fields, pending UCI edits and an unqualified vendor boot adapter.
Physical G4 checks refused a wrong radio binding and a real pending edit while
leaving committed radio files and activation journals unchanged.

The plan changes only the selected Morse radio/interface and mesh policy. It
detaches that interface from IP bridges and leaves normal Wi-Fi, Ethernet, DHCP
and firewall configuration outside the batch. Vendor netifd owns applying the
module options at boot; Hopspot owns packet binding readiness and recovery.
The public [Morse netifd adapter](https://raw.githubusercontent.com/MorseMicro/morse-feed/main/essentials/netifd-morse/lib/netifd/wireless/morse.sh)
illustrates that ownership, but current upstream source is not assumed to match
these installed images. Planning requires the exact inspected adapter digest.
That fingerprint does not attest the entire vendor image or closed firmware.

For these installed MMRC versions, `fixed_bw=3` encodes 8 MHz. Actual Morse
readback, rather than synthetic `iw` VHT labels, verified 924000 kHz and 8 MHz.
Power saving and short-GI rate control were disabled. The unicast preset does
not force the same rate on physical broadcast. The Heltec multicast-rate-control
parameter is outside these boot adapters' exposed configuration contract and
was not overridden.

An independent, persistent 1,200-second procd lease protected each temporary
installation before the radio commits. The qualification used private UCI staging
and separate wireless/mesh11sd commits, followed by the named radio reload.
Application confirmation and radio recovery are separate operations. The existing
release-key trust boundary remains intact: the lab manager is a separate example
outside the shipping bundle. No public release was signed or firmware flashed.

## Verification and restoration

The simulator now exercises delayed radio startup across retained app restarts
and broadcast-only delivery despite accepted unicast sends. It checks typed path
timeout, continued wired control/page access and recovery without restarting the
target. Different physical gateway entries use different fixture connection
identities, matching production TCP address-derived identity. This fixed a fixture
alias exposed by the new scenario; it is not a production routing change.

All 144 routine cases passed twice, and twelve recovery scenarios passed across
four seeds twice. The eight medium owner tests passed. All fifteen appliance
library tests passed on macOS and each real MIPS/Linux board; command and separate
qualification-example tests also passed. Appliance and simulator Clippy passed
with warnings denied, along with formatting, diff and validation-registry checks.
The expanded 1,152-case campaign was not rerun for these additions.

The changed software owners were checked on macOS arm64 with:

```console
cargo test --locked -p personal-hopspot-appliance --all-targets
cargo clippy --locked -p personal-hopspot-appliance --all-targets -- -D warnings
cargo test --locked -p prns-simulation --features controlled-time --lib halow
cargo test --locked -p prns-simulation --features controlled-time --test halow -- --nocapture
cargo clippy --locked -p prns-simulation --features controlled-time --lib --test halow -- -D warnings
cargo fmt --all -- --check
git diff --check
python3 validation/run.py verify
```

The MIPS library test executable used the pinned build toolchain, static target
flags and `-Z build-std=std,panic_abort`; its exact digest is in the JSON report.
Each board executed it from RAM with `--test-threads=1`. This executes the real
Linux filesystem owner tests, not the macOS simulator's virtual packet medium.
`./tools/prns verify` retains its previously reported three network-lab script
placement failures; this change does not claim that check passes.

The reboot harness reconciled actual new UUIDs before its qualifying checks.
Early successful probes can belong to the old process before reboot completes;
they are not boot-latency measurements. Management DHCP reacquisition, probe
duration and invalid board RTC dates also prevent a trustworthy startup benchmark.
Snapshots of RSS, descriptors and overlay space are recorded separately from
sustained resource bounds. Vendor LED/button services remained active.

The G4's shortened recovery lease expired automatically, stopped the app and
restored its original radio files. The Heltecs used the same restoration body.
All seven original vendor configuration/module hashes matched afterward and
pending UCI changes were empty. Morse health passed, but the G4 and second Heltec
AP interfaces remained down after the warm mesh-to-AP change. A clean vendor
reboot recovered the G4 AP; the first Heltec then reassociated at 924 MHz / 8 MHz.
The second Heltec also required a clean vendor reboot to recover its 908 MHz AP.
The lease itself restored files and stopped the app; the subsequent clean reboots
were explicit operator actions. A shipping recovery owner must include and verify
that restart boundary rather than claim the lab lease alone restored operation.
The isolated apps, services and device-side test files were removed. Private
vendor/state backups remain in ignored, mode-0700
`scratch/halow-radio-private-20261002`; credentials and private identities are
excluded from this report.
The host default internet route remained on `en1`, and direct G4 HTTP returned
the normal unauthenticated LuCI login form. Its 403 status is the login gate;
an authenticated browser session was not retested.

The next installation slice is a durable radio transaction owner: validated
preflight, private backup, staged changes, reboot-surviving recovery and explicit
health confirmation and operational vendor-radio recovery. Two UCI commits are not atomic. Interrupted flash/electrical
power-loss recovery, sustained state growth, least privilege and larger/forced
multi-hop meshes remain gates before a public Apply button. Browser/local-helper
transport should present that owner's progress rather than duplicate it.
