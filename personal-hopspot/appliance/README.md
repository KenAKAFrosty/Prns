# OpenWrt application slots

This development slice owns signed application activation and supervised launch
on the inspected ThinkNode G4 and Heltec HT-HD01-V2 vendor images. A separate
durable owner provides protected radio activation and recovery. Explicit bounded
qualification can provision an initial controller grant through the signed app;
service launches never carry enrollment arguments. The manager does not flash
firmware, automatically announce, or update itself. A public installer is still gated on signing operations,
the remaining physical recovery checks and a complete guided enrollment flow.

Build with `./tools/prns run build.hopspot.appliance -- --zig PATH --output NEW_DIR`.
The shipping `manager` pins `release/keys/minisign.pub`. The separately requested
`--qualification-output PATH` builds an example accepting a temporary lab key;
it is never included in the bundle. Do not install that example for users.

The [guided installation procedure](docs/guided-installation.md) is shared by the
website and development bundle. `inspect --profile PATH` emits read-only JSON
compatibility/resource/boot information and a qualified plan without creating
state. `qualify` executes the verified active slot with explicit controller access
and a 1–300-second application lifetime, using the same durable launch accounting
as `run`. It does not confirm health or modify radio configuration. The target
key comes back through the trusted SSH session; the controller pins it before
end-to-end checks. A retained grant can override initial provisioning, so updates
preserve state and do not reenroll merely to recover access.

## Package contract

Each private package directory contains `manifest.json`, its detached Minisign
signature `manifest.minisig`, and `app.gz`. The signed JSON has schema 1, a
path-safe `version`, one exact vendor `board`, ABI
`mips32r2-le-o32-soft-float-static`, and `compressed` / `executable` objects with
positive `bytes` and lowercase SHA-256 `sha256`. Verify the signature before
parsing metadata, then verify both artifact hashes, expanded length, static
ELF load headers and the MIPS soft-float ABI flags. Decompression is bounded by
the signed executable length and explicit local budgets.

An inspected root contains a fixed `manager`, an explicit `config.json`, slots
`a` / `b`, alternating checksummed journals and `state/`. The application alone
owns state identities and retained authorization. Updates never delete or copy
that state. Provision initial grants deliberately through the existing headless
CLI before enabling the service; the durable launch configuration cannot carry
enrollment arguments that would undo later revocation. Copying private state
between different appliances is prohibited.

## Guided activation

Keep private configuration/state backups and independent wired management. First
inspect `/tmp/sysinfo/board_name`, filesystem/RAM budgets and existing listeners.
Use the same explicitly supplied budgets for `stage`, `status`, `confirm`,
`rollback` and `run`; the example procd asset shows the minimal application's
current limits. It is a profile to qualify, not a universal device budget.

1. Upload a verified package into private RAM. `stage --package PATH
   --trial-launches 3` verifies it under the manager's trust and board policy,
   checks storage headroom, replaces only the inactive slot and publishes a
   durable trial journal. It returns the candidate revision and digest as JSON.
2. Install a reviewed launch config and the service asset. `run --config PATH
   --ram-directory /tmp/hopspot` consumes a durable trial launch before expanding
   the executable. It then uses Unix `exec`, preserving procd's PID and signal
   ownership. No radio presence is required for wired application readiness.
3. Through a pinned controller, verify actual build/interface snapshots, an
   authenticated app exchange and the exact node page/Resource. Inspect radio
   availability separately. `confirm --revision N --executable-sha256 DIGEST`
   accepts only that exact current trial. Confirmation is a trusted operator
   action following those checks; the manager does not infer health from a PID,
   socket, log line or association.
4. A failed candidate can be rolled back explicitly. Three unconfirmed launches
   exhaust the trial and select the verified previous application on the next
   launch. With no previous application, startup fails closed. Confirmed packages
   are reverified at every launch. A corrupt confirmed slot is never executed.

The service passes an explicit `respawn 60 2 3`: quick failures have bounded
retries, while a process surviving the threshold starts a fresh crash window.
Trial launch accounting also persists across boots. `term_timeout 15` permits
the application's SIGTERM flush. File descriptors are capped at 128 and core
dumps disabled. There is no netdev trigger that restarts the application during
radio churn. Root/CAP_NET_RAW and OS log rotation remain separate qualification
gates; do not claim least privilege from this service file.

## Recovery and evidence boundaries

Files are synchronized before publication; slot and journal renames are followed
by directory synchronization. Interrupted candidate staging cannot replace the
confirmed slot. Journal checksums detect torn JSON; the highest valid revision
wins. Two corrupt journals, or vanished journals beside installed slots, refuse
startup. A failed write poisons the writer until it is reopened, because a failed
directory sync may have followed a visible rename. Concurrent writers are locked.

Native cut-point tests model interruption after each observed publication step,
trial exhaustion, stale confirmation, corrupted packages/journals and unchanged
state. They are filesystem/process fault evidence, not actual flash power cuts.
The filesystem transaction is qualified on Unix filesystems; Windows browser or
local-helper clients delegate activation to the appliance's Linux manager. The
signature/package contract and activation vocabulary remain portable Rust.
Real power-loss behavior, manager upgrades, anti-downgrade policy, signing-key
rotation and a browser/local-helper transport remain subsequent gates. The app
signature authenticates publisher intent; the local journal checksum is not a
signature or protection against an attacker with root access.

## Persistent radio preparation

`radio-plan --profile PATH` reads a bounded, explicit radio profile and returns
JSON containing a UCI batch. It does not open application slots, change radio
settings, commit UCI, or restart services. Supply the manager's normal explicit
resource arguments. `openwrt/radio-profile.example.json` selects the current
US-only 924 MHz / 8 MHz, fixed MCS2, long-guard, 18 dBm preset. This is an explicit
regional selection, not a worldwide default. Other regions and vendor boot
adapters require qualification before they can produce a plan.

The planner verifies the inspected board's exact `morse.sh` digest, named UCI
section types, Morse radio type, interface-to-radio binding and absence of
pending operator UCI changes. It only targets that radio/interface and the mesh
forwarding policy. Names cannot inject UCI commands; unrelated capabilities and
the normal Wi-Fi, Ethernet, DHCP and firewall configuration stay outside the
plan. The profile makes an open mesh, detaches it from IP bridges and disables
mesh data forwarding and gateway announcements so PRNS owns onward routing. It
retains proactive HWMP path requests (`mesh_hwmp_rootmode=2`): established mesh
peers alone did not reliably populate the kernel's unicast paths after cold boot.
These are 802.11 management frames, separate from application announcements.
MAC-derived peer identity and cryptographic enrollment remain separate.

The vendor netifd adapter already accepts fixed-rate module options, regenerates
`/etc/modules.d/morse` and reloads the driver when required. OpenWrt owns boot
application of those settings; Hopspot owns radio availability and rebinding.
The qualified older MMRC contract encodes 8 MHz as `fixed_bw=3`, not `8`. The plan
also disables module power saving and advertises long guard intervals. It does
not add a radio poller or an automatic application announcement. The Heltec's
optional multicast-rate-control parameter is not exposed by these boot adapters
and is deliberately not overridden: group announcements retain vendor group-rate
behavior, separately from the fixed unicast rate.

Application confirmation and radio confirmation are separate transactions. The
radio owner provides `radio prepare`, `apply`, `confirm`, `rollback`, `status`,
`recover` and `watch`. It owns one device-wide private journal at
`/etc/hopspot-radio`, independently of application slots, identities and grants.
A different application root cannot open a second overlapping radio trial.
`prepare` keeps originals of wireless, mesh11sd, system and the generated Morse
module file, including permissions. It generates the candidate in private UCI
files in RAM; no live UCI commit or radio reload occurs during preparation. It
checks every intended candidate value, removed key and long-guard list member
after staging. A successful UCI batch exit alone cannot qualify a candidate.
The originals contain credentials: retain recovery exports privately.

Install the reviewed `openwrt/hopspot-radio-recovery` asset as
`/etc/init.d/hopspot-radio-recovery`, make it executable, then enable it. The asset
expects the manager at `/etc/hopspot/manager`. It starts independently of the
application, before the vendor network service at boot. `prepare` starts this
service; `apply` requires both its boot link and a running procd instance using
the same manager root. Missing protection and pending UCI edits refuse activation
before changing vendor files. Recovering originals ignores and clears pending
edits to the owned packages so stale UCI deltas cannot override restoration.
Inspection and staging use absolute UCI file paths, which bypass delta loading;
`-t` alone changes the save directory but retains other delta search paths in
[libuci](https://github.com/openwrt/uci/blob/master/delta.c).

```sh
manager() {
    /etc/hopspot/manager --root /etc/hopspot \
        --max-compressed-bytes 2097152 --max-executable-bytes 4194304 \
        --flash-reserve-bytes 262144 --ram-reserve-bytes 8388608 "$@"
}
chmod 700 /etc/init.d/hopspot-radio-recovery
/etc/init.d/hopspot-radio-recovery enable
manager radio prepare --profile /etc/hopspot/radio-profile.json \
    --recovery-seconds 180 --trial-boots 1
```

Read `revision` and `profile_sha256` from the returned JSON, then provide those
exact values to `radio apply`, `confirm` or `rollback` with `--revision` and
`--profile-sha256`. The profile example is explicitly US-only; inspect the board,
SKU, operating location, wired management path and storage first. A stale
candidate cannot confirm another preparation. `rollback` cancels an unapplied
preparation immediately; an applied trial is restored by the independent service.
`status` reports its phase as JSON. `recover` performs one recovery step; it can
restore files and request a reboot.

The lease uses the kernel boot UUID and uptime, not the boards' unreliable wall
clocks. Each permitted trial reboot consumes persisted allowance and gives one
new bounded window. Normal checks do not renew the deadline or write the journal.
A mutation-intent record is synced before the first vendor file change. Journal
and backup integrity checks prevent a damaged Applying record from concealing
partial activation. Lost journals with an intact guard select restoration;
damaged backups refuse host writes. Recovery restores all originals atomically
per file, preserving modes, then requests a clean vendor reboot. It requires a
changed boot UUID, matching original files, interface UP/carrier and Morse health
before reporting `Restored`. A missing reboot has a 60-second deadline; radio
readiness has a separate 360-second deadline. Unavailable recovery remains an
explicit nonterminal state, blocking a new trial. Restoring bytes alone is not
reported as operational recovery.

The local manager has no generic shell or automatic application announce policy.
It deliberately does not auto-confirm a trial. The trusted installer/controller
must judge end-to-end health and confirm the exact candidate. This is a development
transaction owner; physical flash power-loss qualification remains required
before an unattended public Apply flow.

Confirm health through a different live mesh gateway after boot: authenticate
against the existing pinned target, inspect build/interfaces/config/peers, verify
an app message and exact page bytes, and read the real Morse channel/rate and
mesh parameters. Explicit controller announcements may seed discovery. Use the
actual PHY associated with the Linux device, because module reload can change
its index. Keep the wired management listener's address family explicit; IPv6
link-local management needs an IPv6-capable bind and interface scope. Confirm
the exact radio candidate only after these checks, or allow the lease to restore
the original vendor configuration. The separate qualification service is not a
shipping installer.

The [three-board mesh boot qualification](../headless/docs/qualification/halow-mesh-boot-2026-10-02.md)
records real reboot, over-air snapshots/messages/pages, simulator coverage and
the lab recovery lease. Its mesh-to-AP restoration needed clean vendor reboots on
the G4 and second Heltec after matching files and Morse health had already passed.
The durable radio owner now owns that restart boundary and verifies operational
recovery; the [radio owner qualification](../headless/docs/qualification/halow-radio-owner-2026-10-02.md)
records the exercised scope and remaining gates.
