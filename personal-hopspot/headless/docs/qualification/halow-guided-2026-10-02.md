# Guided HaLoW application installation qualification

The G4 and both Heltecs passed the guided manager flow with the same final static
MIPS artifacts. Read-only compatibility/resource inspection created no application
or radio state. The shipping manager refused the lab signer. The separate lab
manager staged the board-specific signed package, ran explicit bounded controller
enrollment, and served authenticated build/interface snapshots, the verified
controller probe and the exact 3,542-byte page. A subsequent normal manager launch
carried no enrollment arguments and retained the same pinned target and grant.
Only after those health checks was the exact application candidate confirmed.

[Structured evidence](halow-guided-2026-10-02.json) contains public receipts,
artifact hashes, measured resource snapshots and seven unchanged vendor hashes
per board. Private signing keys, node/controller secrets and configuration backups
are excluded. All application slots/state/execution in this slice were temporary
RAM deployments; no reboot or new over-air campaign ran. Earlier persistent
service and radio qualifications remain separate evidence.

## Guided owner and provisioning boundary

The website renders one shared
[installation procedure](../../../appliance/docs/guided-installation.md) after
the selected model's compatibility information. The manager development bundle
contains that exact same document for offline use. Model pages no longer describe
supervised service and protected radio activation as wholly unimplemented.
Public downloads remain gated; an unsigned manager checksum list authenticates
neither its publisher nor its bootstrap trust.

`inspect --profile PATH` emits schema-1 JSON with the inspected board, kernel,
boot UUID/uptime, actual overlay and RAM availability, explicit budgets, signer
text digest and qualified radio plan. It reuses the existing plan and vendor time
owners before opening any application or radio journal. Available RAM is the
smaller of tmpfs capacity and Linux MemAvailable; missing/malformed measurements
fail rather than becoming invented availability. Inspection is not a reservation
or whole-image/regional attestation.

`qualify` uses the signed slot owner's existing verification, expansion and trial
accounting before Unix exec. Controller public key, permission selection and
1–300-second application lifetime are explicit typed inputs. Initial inspection
also permits controller-operated AnnounceSelf; app probe and interface watch each
add their own separate permission. No enrollment argument is stored in the normal
launch config. A restart cannot automatically reenroll a revoked controller.
Retained authorization remains authoritative, and updates must preserve it.
The manager cannot infer successful provisioning from the printed initial-grant
count; actual authenticated traffic is the proof.

## Hardware observations

Each target was identified by pinned SSH, board name, distinct radio MAC and
boot UUID. The second Heltec used its original scoped IPv6 link-local management
connection and an IPv6 application listener. No factory password/address was
added to source or installer instructions. Each board generated separate private
application state; it was not cloned from another board.

All three passed wrong-signer refusal, signature/board verification, real
controller identity delivery, exact page fetch and graceful stop. After the
qualification launch, the three-attempt trial reported two attempts remaining.
Normal `run` then consumed the next trial attempt without enrollment flags.
The same pinned target and probe still worked, demonstrating retained grants.
Confirmation returned the original exact candidate in `Confirmed` state.

The first G4 harness check read its log after two seconds, before expansion/core
readiness finished. The existing candidate was inspected and checked afterward;
staging was not replayed. The final harness waits for actual readiness within a
bounded window. No startup-latency or throughput claim follows from this check.
Transfer completion and readiness remain distinct local stages.

Original network, wireless, firewall, DHCP, mesh11sd, system and generated Morse
file hashes matched afterward on all three. Pending UCI changes were empty,
Morse health passed, temporary application roots were removed and no radio owner
or recovery service was installed. The computer's Internet route remained on
Wi-Fi `en1` through `192.168.4.1`.

## Checks and remaining gates

- `cargo test -p personal-hopspot-appliance --all-targets`: 31 library tests,
  seven shipping CLI tests and seven separate lab CLI tests passed on macOS arm64.
  New checks cover explicit bounded/key policy, permission argument delivery,
  service launches without enrollment, absent/malformed memory measurements and
  rejection of the lab observation option by the shipping CLI.
- Appliance all-target Clippy with `-D warnings`, package/site formatting and
  diff checks passed. `cargo test --locked --manifest-path docs/website/Cargo.toml
  --lib` passed all 48 site tests, including compilation of the shared guide pages.
- The qualified Rust/Zig builder produced shipping manager 1,511,340 bytes,
  SHA-256 `7aa5de63d070f2a3111eb1088b95b2dc7d3bdcdd51236b916b5b61ad1ed16577`,
  and separate lab manager SHA-256
  `1c681c174a5c1f89b3fd2aff778888fb83bddc2c9e922f3d263bdf87f672bf3c`.
  Bundle checksums and the shared guide matched. The electrical checkpoint marker
  occurs only in the lab executable. Both final artifacts ran on all three boards.
- `python3 validation/run.py verify` and `./tools/prns verify` passed. Separate
  tooling commit `5e2572a97` moved the preexisting network-lab implementations into
  their registered tooling owner without changing their bytes. All 23 task-runner
  tests, AP/client private staging and shell syntax passed.

The [electrical qualification procedure](../../../appliance/docs/power-loss-qualification.md)
is prepared with a separate lab-only, 90-second checkpoint after wireless
publication and before mesh11sd publication. The shipping manager has no such
option. A fresh private G4 vendor backup and original hashes/modes were retained.
No checkpoint trial was armed and no electrical cut happened in this slice.
Electrical outcomes require an available physical operator and their own evidence;
neither the preparation nor earlier filesystem fault tests count as that proof.

Signed manager/bootstrap distribution and upgrade handling remain required before
public installation. Automated browser/helper transport, further electrical
checkpoints, other regional/vendor profiles, least privilege, sustained resource
growth/load, forced multi-hop and field range remain separate gates. The current
result is an exercised guided development path through the existing owners, not
an unattended public installer or another browser-owned radio state machine.
