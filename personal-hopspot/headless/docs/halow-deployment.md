# HaLoW appliance deployment

The first user experience should start in the web installer: choose the exact
ThinkNode G4 or Heltec HT-HD01-V2, download a verified Linux application bundle,
and follow a guided Ethernet/SSH installation. Keep the vendor operating system,
Morse firmware, regulatory configuration and calibration. These are application
targets, separate from firmware-upgrade targets.

The [application slot manager](../../appliance/README.md) implements the first
activation and supervised-service slice. It is not a public installer. The
[initial desk qualification](qualification/halow-appliance-2026-10-01.md)
records working transport/control behavior and the recovery gap found on hardware.
The [recovery qualification](qualification/halow-recovery-2026-10-02.md) closes
that demonstrated binding/discovery gap on all three boards, with bounded retry
and retained identities. The [persistent service qualification](qualification/halow-service-2026-10-02.md)
adds real reboot, retained control and exact pages on all three, plus G4
crash-driven rollback. The [mesh boot qualification](qualification/halow-mesh-boot-2026-10-02.md)
adds the explicit persistent radio profile and over-air checks after verified new
boot epochs. The [durable radio owner qualification](qualification/halow-radio-owner-2026-10-02.md)
adds protected activation, automatic operational rollback and pending-UCI fault
recovery on all three boards. Physical power-loss recovery remains a gate.

## Compatibility and budgets

| Property | ThinkNode G4 inspected | Two Heltec HT-HD01-V2 inspected |
| --- | --- | --- |
| Board identifier | `morse,ekh03v3` | `Heltec,HT-HD01-V2` |
| CPU/application ABI | MT7628AN; static MIPS32r2 little-endian O32, soft float | Same |
| Kernel | 5.15.150 | 5.15.167 |
| Vendor image | OpenWrt 1.1 Morse-2.6.13 | OpenWrt 23.05.5, 2.8.5-20250924 |
| Writable overlay available at inspection | 5,336 KiB | 5,288 / 5,272 KiB |
| Recovery-qualified application | 3,636,552 bytes | Same executable and hash |
| Cold-boot corrected application / gzip | 3,631,976 / 1,507,694 bytes | Same |
| Guided installation shipping manager | 1,511,340 bytes | Same |
| Earlier candidate gzip, level 9 | 1,501,636 bytes | 3,620,680-byte candidate; unpacking tested on G4 only |

Two compressed slots with RAM expansion now passed service/reboot qualification;
two unpacked executables do not fit these overlays. The latest manager and guided
flow are measured in the [guided qualification](qualification/halow-guided-2026-10-02.md);
that slice uses RAM slots and does not repeat persistent footprint qualification.
Do not assume a newer application or another vendor
image has the same budget. The overlay must also accommodate private state,
configuration, update metadata and filesystem overhead. Updates need staging
headroom in RAM and explicit low-space refusal before writing flash.

G4 RAM-only gzip expansion, SHA-256 comparison and executable version invocation
passed. The board's coarse seconds counter advanced by two seconds across
expansion. This is not a boot-time benchmark or power-loss/update qualification.
Compression is storage encoding, not signature verification. Signed metadata must
identify both compressed and unpacked hashes and exact compatible board profiles.

## Installation transaction

One installer engine should own these named states; the website and a local
helper present its structured progress rather than reimplementing the transaction.

1. **Inspected:** read the board identifier, image/kernel, CPU ABI, regulatory
   region, radio devices, wired management path, listeners and actual RAM/overlay
   budgets. Reject unsupported combinations and pending vendor configuration edits.
   Discover addresses; do not bake in a factory IP or password. Verify SSH host keys.
2. **BackedUp:** export configuration and existing private Hopspot state privately.
   Record hashes and a restoration procedure; do not include credentials or board
   calibration in shared qualification output.
3. **Staged:** verify release signatures and compatibility locally, upload into a
   new private RAM directory, verify hashes on-device, then expand and verify the
   executable. Partial uploads never become an active version.
4. **Qualified:** arm an independent rollback lease before changing radio settings.
   Preserve Ethernet management. Start a bounded candidate with separate state and
   narrow lab grants. Require a real authenticated control exchange and an exact
   Resource/page exchange; a PID, listening port or mesh association is insufficient.
   Restoration also needs an operating interface: Morse health and matching
   configuration files did not prove recovery after a mesh-to-AP transition.
5. **Activated:** persist a verified version, configuration and supervised launch
   transaction. Keep identity and authorization state outside replaceable slots.
   Resolve radio-device readiness and recreation before claiming appliance readiness.
6. **Verified:** check the active application, unchanged identities, control/page
   exchanges and the retained previous version. Cancel rollback only after these
   checks. A failure restores the previous version and radio configuration.

Installation, updating and recovery use this same owner. No generic remote shell
operation is added to PRNS Remote Control. Avoid sourcing an untrusted downloaded
shell script or accepting arbitrary helper commands. Offline users should be able
to download the complete bundle and perform the same checks through documented SSH
steps. A failed health check needs a locally named stage; a timeout does not prove
an authorization denial.

## Radio and controller configuration

The current US lab profile is open 802.11s, 924 MHz center, 8 MHz, MCS2, long guard
interval, power saving off and mesh forwarding off. Proactive HWMP path requests
remain enabled (`mesh_hwmp_rootmode=2`); otherwise one cold-booted board received
broadcast but had no usable kernel unicast paths. Gate announcements remain off.
HWMP management traffic is separate from application announcements. Region and
power need an explicit installation choice; the desk run used 18 dBm. Check the
regional SKU and operating location before using the US-only plan; its adapter
fingerprint does not attest regional legality or the whole vendor image.
Keep the profile fixed and editable. Do not silently apply the US channel to a
different regional SKU. Vendor `iw` output uses synthetic VHT channel/rate labels;
validate frequency/width through Morse tooling, without continuous intrusive polling.

The manager's typed, read-only `radio-plan` preflights the exact installed vendor
boot adapter and named binding before returning a reviewable UCI batch. It does
not activate the radio or confirm recovery. The
[radio preparation procedure](../../appliance/README.md#persistent-radio-preparation)
keeps planning separate from the durable radio owner. Its private device-wide
journal, independent boot service, explicit bounded lease and exact-candidate
confirmation now own activation and rollback. Operational recovery requires a
new boot epoch and an operating radio; file restoration alone cannot pass.

Persist one explicit, stable local HaLoW scope across interface renames and
application updates. Source MAC supplies immediate neighbor identity without a
station-list polling dependency. MAC identity is transport addressing; controller
and node authentication remain cryptographic. Never clone private node state
between appliances.

The Linux supervisor now retires packet bindings on device down/delete or lost
link notifications and reopens the named device with bounded jittered backoff.
It retains the configured parent interface ID and reconstructs its child lanes.
A missing device at cold boot or during operation leaves wired management available;
application readiness no longer waits for a broadcast radio child. `Connected` describes a
packet binding, not mesh association or successful delivery. Keep route recovery
separate: configured gateways can rediscover unavailable routes without wiping
retained state. Applications still handle typed operation timeouts and explicit
retries; reconnection does not silently replay an application command.

Use `--tcp-mode gateway` when connected wired clients need discovery of destinations
behind the appliance. PointToPoint remains the CLI default. Gateway changes
discovery forwarding, not authorization. It does not establish that cold discovery
will cross every multi-hop topology; that needs separate routing qualification.

Enroll the controller's full public key and return the target's public key through
the trusted installation connection. Pin the target key in the controller. Give
inspection explicitly; app messages and interface watch each require separate
grants. Private controller keys remain on the controller. Retained grants require
deliberate revocation; omitting a CLI key is not a general revocation mechanism.

There is no automatic application announcement policy. For an entirely fresh lab,
the controller explicitly announces each node page through its wired connection.
That seeds HaLoW neighbors. Subsequent control can route across the radio; cold
control-endpoint discovery was qualified with these neighbors established. A quiet
mesh association alone does not populate PRNS MAC peers. Announcements, including
ordinary relays, use the shared broadcast channel; directed traffic uses unicast.
Repeat explicit discovery seeding as needed after reboot. Installation verification
should first reconcile the actual new boot epoch, then check the radio and fetch
authenticated snapshots and the exact page through another live gateway. An early
successful response may have come from the process about to reboot.

## Web-led entry and later automation

The website embeds one [shared guided procedure](../../appliance/docs/guided-installation.md)
below each exact model’s compatibility information. The manager bundle carries
that same document. Read-only `inspect` snapshots and explicit bounded `qualify`
launches make the steps executable through the existing owners. Public signed
downloads and automated browser/helper transport remain gates.
The USB Ethernet adapters here expose a network connection, not the remote board's
flash. [WebUSB needs an available claimable interface](https://developer.chrome.com/docs/capabilities/build-for-webusb);
the observed running G4 exposes no USB device-controller interface.

Browser-driven installation is still plausible through an authenticated device
endpoint or a local helper that runs the same installer engine. This adds its own
origin, credential and session-token contract. Chrome's
[Local Network Access permissions](https://developer.chrome.com/blog/local-network-access)
can gate public-site requests to LAN/loopback destinations; browser support and
transport-specific restrictions need actual matrix tests. Those permissions grant
network access, not installation authority. A public WebSocket Reticulum listener
must not become the installer endpoint.

## Remaining qualification gates

- Binding replacement and retained-route rediscovery passed on the G4 and both
  Heltecs with the process still running. One page handshake required an explicit
  retry; Ethernet capture did not establish the radio/firmware loss mechanism.
  Extend this bounded desk check with repeated service/radio lifecycle and longer
  resource monitoring before calling the persistent appliance qualified.
- Procd start/stop, real reboot, pinned identities/retained grants and exact wired
  pages passed on all three. G4 trial exhaustion/rollback and bounded corrupt-identity
  crash termination passed. Cut-point tests run on macOS and all three actual MIPS
  filesystems. The temporary qualification loader trusts a separate lab signer;
  the shipping manager rejects that signer. No public release signing was performed.
- Qualify physical interrupted flash/power-loss recovery, sustained state growth,
  and manager/bootstrap upgrade handling. The explicit US mesh/rate
  profile and controller-seeded over-air reboot checks now passed on all three;
  other regions, vendor adapters and unattended cold discovery remain unqualified.
  Warm mesh-to-vendor-AP restoration required clean vendor reboots on the G4 and
  second Heltec. The durable owner now enforces that clean reboot and operating
  interface boundary; matching files and a successful firmware health query alone
  cannot complete recovery. Do not
  discard authorization snapshots merely to avoid route writes.
- Qualify least privilege, bounded logs/state, sustained actual PRNS Resource
  throughput, longer desk runs, physical forced multi-hop and field range.
  Vendor LED/button polling remains active in the latest desk checks; replacing
  it needs preserved button behavior, not a shipped `SIGSTOP` trick.
- Keep browser assets, Auto-WiFi and WebSockets optional until their additional
  flash/RAM and coexistence budgets fit. ESP-NOW interoperation is a separate
  unproven capability on these Linux radios.
