## Guided application installation

This is a development procedure for the inspected vendor images. Public downloads
remain gated on signed manager/bootstrap distribution and physical power-loss
qualification. The website offers these steps without an unattended Apply button.
Keep Ethernet management throughout. The application installer does not replace
the vendor operating system, calibration or Morse firmware.

### 1. Inspect and back up

Discover the actual management address using your Ethernet connection and vendor
panel. Verify the SSH host key through a trusted connection. For IPv6 link-local,
include the computer's Ethernet scope in the SSH address and controller endpoint.
Keep your Internet connection on a separate interface. Neither an Ethernet USB
adapter nor a browser network permission grants installation authority.

Before uploading an executable, inspect the exact vendor board name:

```sh
cat /tmp/sysinfo/board_name
cat /proc/sys/kernel/osrelease
df -Pk /overlay /tmp
uci -q changes
```

Compare the board and image with the model guide. Resolve pending UCI changes
deliberately. Export the vendor configuration using its backup facility, download
it to a private directory, and record its hash. If updating, gracefully stop
Hopspot and privately export its whole application root and `/etc/hopspot-radio`
before proceeding. Record original service enablement and your wired restoration
path. Backups can contain passwords and private identities; never share or clone
them onto another board. Do not restore arbitrary archives over a running node.

### 2. Verify the artifacts and transfer them privately

Use a trusted, exact-source development manager build or an authenticated release
bundle. Its checksum list detects transfer errors; an unsigned checksum list does
not authenticate its publisher. The shipping manager pins the repository release
key. The separately built lab manager accepts a lab signer and never belongs in a
user bundle. Never upload either bundle through LuCI firmware upgrade or `sysupgrade`.

A signed application package contains `manifest.json`, `manifest.minisig`, and
`app.gz`, and names this exact vendor board. Keep a copy on the computer. Transfer
the package and reviewed manager assets into a fresh mode-0700 RAM directory.
Use your SSH client's binary transfer facility; some vendor SSH servers do not
provide SFTP. Transfer to new names, verify hashes on the device, then publish
complete files. If an upload times out, inspect its actual hash and destination
before retrying. Do not overwrite an installed manager during an active trial.
Manager updates need a separately qualified bootstrap/update procedure.

In the following commands, `/etc/hopspot/manager` is the reviewed installed
manager, `/tmp/hopspot-package` holds the transferred package, and the supplied
budgets are the inspected desk profile. Recheck headroom for your build. The
manager enforces each signed package and reserve independently; installing the
manager itself also consumes flash.

```sh
manager() {
    /etc/hopspot/manager --root /etc/hopspot \
        --max-compressed-bytes 2097152 --max-executable-bytes 4194304 \
        --flash-reserve-bytes 262144 --ram-reserve-bytes 8388608 "$@"
}
manager inspect --profile /tmp/hopspot-upload/radio-profile.json
```

`inspect` emits one JSON snapshot with the board, kernel, boot UUID/uptime, actual
available overlay/RAM, explicit budgets, signer-text digest and qualified radio
plan. It refuses unsupported boot adapters, bindings and pending UCI changes.
It creates no application or radio state. Its signer digest is identification,
not proof that an untrusted manager is authentic. Inspection does not prove a
SKU's regional legality or make a snapshot a reservation.

### 3. Stage the application and enroll your controller

Install a private launch config with an explicit listener, Gateway TCP mode and
stable HaLoW scope. An IPv4 listener accepts IPv4; an IPv6 management path needs
an IPv6 listener. For example:

```json
{
  "listen": "[::]:4242",
  "tcp_mode": "Gateway",
  "radio": { "HaLow": { "device": "wlan0", "scope": "primary-halow" } }
}
```

Keep that scope through updates and interface renames. Set `radio` to `"Disabled"`
for initial wired-only qualification. The listener carries Reticulum, not HTTP.
Install the config at `/etc/hopspot/config.json`, mode 0600. It contains no
controller grants, automatic announcement policy or arbitrary executable options.

```sh
manager stage --package /tmp/hopspot-package --trial-launches 3
```

Save the returned application candidate's `revision` and `executable_sha256`.
The manager verifies publisher signature, board, lengths, hashes and static
soft-float ABI before publishing the trial. A disconnected acknowledgement means
completion is uncertain: run `manager status` and reconcile the candidate before
restaging. Staging never replaces the confirmed slot or private state.

On your computer, create or load the controller identity with the repository's
headless `controller` example. Keep its private directory on the computer:

```sh
controller --state-dir /private/controller-state identity
```

Copy only its full 128-character `public_key` into `CONTROLLER_PUBLIC_KEY` in the
trusted device shell. On first enrollment, while the application service is
stopped, run the signed slot through the manager:

```sh
manager qualify --config /etc/hopspot/config.json --ram-directory /tmp/hopspot \
    --controller-public-key "$CONTROLLER_PUBLIC_KEY" \
    --controller-access application-probe --run-for 180
```

This consumes one of the trial's three launches and preserves procd/Unix exec
signal ownership. It exits gracefully after the specified application lifetime
(1–300 seconds). Choose `inspection` for inspection plus explicit AnnounceSelf,
`application-probe` to additionally test the bounded app message, or
`interface-watch` to additionally permit the bounded interface stream. Neither
extra grant implies the other. Private controller keys never leave your computer.

Read and retain the target's full `target_key` and node-page destination from its
ready output through this trusted SSH session. The effective retained grant is
proven by a successful authenticated operation, not the printed initial-grant
count. Previously retained authorization can override initial grants. An update
must preserve it; do not rerun enrollment or delete state to fix an access timeout.
An already enrolled update can use the normal service launch and existing pinned
controller. Grants persist outside the slots; service restarts cannot reenroll a
revoked controller.

### 4. Prove health before confirming

While the candidate runs, use the wired endpoint and pinned target key on your
computer. Replace the endpoint with the actual IPv4 or scoped IPv6 address.
The last command requires the explicitly selected `application-probe` access:

```sh
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" --action build
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" --action interfaces
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" \
    --action app-message --message-hex 0101
```

Fetch configuration and peer pages for actual inventory interface IDs, and fetch
the complete node page through a separate client. Check expected build identity,
real interface availability, verified-controller probe response and exact page
bytes. A PID, socket or radio association does not qualify the candidate. The
controller emits JSON lines and names its local timeout stage; a timeout alone
does not prove denial. Unauthorized requests intentionally remain silent.

After qualification exits, install the reviewed `openwrt/hopspot` service asset
as `/etc/init.d/hopspot`, mode 0700, then enable and start it. It uses the same
signed slot and state, without enrollment or automatic announcements. Check the
same pinned target and page again. Before trial launches are exhausted, confirm
the exact application candidate using your saved values:

```sh
manager confirm --revision "$APP_REVISION" \
    --executable-sha256 "$APP_EXECUTABLE_SHA256"
```

Never substitute the journal revision for the candidate revision. If health
fails, stop the service, use `manager rollback`, then start and verify the previous
application. On first installation there is no previous application; failure is
closed. Three unconfirmed launches exhaust the trial. Confirmation is an explicit
operator action, never inferred automatically from readiness output.

### 5. Activate HaLoW as a separate protected transaction

Review the explicit radio profile for your actual region and SKU. The current
qualified profile is US-only: 924 MHz / 8 MHz, MCS2, long guard, 18 dBm, open
802.11s with PRNS forwarding and proactive HWMP paths. Other regions require
qualification. Keep independent wired access. Install and enable the reviewed
`openwrt/hopspot-radio-recovery` asset before applying any radio candidate.

If initial wired qualification used `"Disabled"`, gracefully stop the application
service, change only the private config's `radio` to the named `HaLow` device and
stable scope shown above, then restart normally. Verify the existing pinned wired
controller again. A temporarily unavailable radio must leave wired control usable;
do not reenroll the controller or delete state during this change.

```sh
chmod 700 /etc/init.d/hopspot-radio-recovery
/etc/init.d/hopspot-radio-recovery enable
manager radio prepare --profile /etc/hopspot/radio-profile.json \
    --recovery-seconds 300 --trial-boots 1
```

Save this separate candidate's `revision` and `profile_sha256`. Preparation starts
the independent service, keeps originals/modes privately and changes no live
vendor files. Apply only when the controller and peer gateway are ready:

```sh
manager radio apply --revision "$RADIO_REVISION" \
    --profile-sha256 "$RADIO_PROFILE_SHA256"
```

Do not shorten the example window without accounting for management reacquisition.
Reboot within the trial only deliberately; the one allowed extra boot consumes a
persisted allowance and starts one new bounded window. Reconcile the actual new
boot UUID before calling it a reboot success. Discover the restored management
address again when necessary; reboot can interrupt Ethernet DHCP.

Through each board's wired controller, explicitly invoke `announce` to seed a
fresh lab's HaLoW neighbors. Verify the intended frequency/width/rate, and fetch
authenticated build/interface/config/peer snapshots, probe and exact page through
another live gateway. Peers and HWMP paths alone do not prove PRNS delivery.
Only then confirm this exact radio candidate:

```sh
manager radio confirm --revision "$RADIO_REVISION" \
    --profile-sha256 "$RADIO_PROFILE_SHA256"
```

If checks fail, request `radio rollback` with those same candidate values, or let
the lease expire. The independent service restores vendor files, clears owned
pending deltas and requests a clean vendor reboot. `radio status` must reach
`Restored` with a new boot, matching original files, carrier and Morse health.
`RebootUnavailable` and `RestorationUnavailable` remain failures requiring wired
investigation. Do not start another trial or remove the recovery guard to bypass
them. App confirmation and radio confirmation never imply one another.

### 6. Retain recovery information

Keep the verified package, private backups, both exact candidate receipts, pinned
controller/target public identities, service/config hashes and verified boot UUIDs.
Remove RAM upload directories after reconciliation. Keep the independent radio
recovery service and its durable guard installed. Graceful updates preserve state
and grants and use the same slot owner. Physical power cuts, manager upgrades,
other regional/vendor profiles, sustained load and field range remain separate
qualification requirements. Browser assets, WebSockets and Auto-WiFi are optional
features with separately measured resource and exposure budgets.
