# Electrical power-loss qualification

Status: prepared procedure; electrical outcomes must be recorded separately from
the passing filesystem/process fault tests. Lead with the G4, then repeat on each
Heltec. Keep independent wired management and private backups. Do not combine
this test with application updates, key changes, firmware flashing or calibration.

## First cut: partially applied radio transaction

The separate `qualification` example accepts
`--qualification-radio-observation pause-after-wireless-replacement`. This option
is absent from the shipping manager and its bundle. It pauses for at most 90
seconds after the owner has synced mutation intent and atomically replaced the
wireless file, before replacing mesh11sd or reloading the radio. The durable phase
is `Applying`; a complete recovery guard already holds all originals and modes.
It prints and flushes a `qualification_power_cut_ready` JSON event. A missing cut
returns an interruption error and releases the owner lock, allowing the independent
watcher to recover. The watcher itself runs without any pause option.

1. Identify the physical unit through pinned SSH, exact board name, radio MAC and
   current boot UUID. Stop on an ambiguous address or identity. Privately export
   fresh vendor configuration, radio-original hashes/modes and any Hopspot state.
   Verify wired recovery and the exact tested manager/hash before changing files.
2. Install an isolated lab manager and private lab public key. Install the
   independent recovery service with its matching root and no pause argument.
   Verify its boot link and running procd instance. Never ship this signer or
   qualification executable. Preserve original service/configuration state.
3. Prepare the explicit qualified US radio profile with a 180-second lease and
   one additional trial boot. Record the exact candidate. Verify all original
   hashes remain unchanged. Capture baseline boot identity and radio health.
4. Once the operator is physically ready, run `radio apply` for that candidate
   using the pause option. Capture the ready event and inspect the sealed journal
   receipt and partial file hashes read-only from the independent SSH connection;
   the paused writer still holds the manager lock. Do not rerun apply
   merely because its completion acknowledgement is absent.
5. Tell the operator that the named G4 is at the checkpoint. Within the reported
   90-second window, remove only its electrical power, leave it off for five
   seconds, then reconnect. Do not substitute reboot, process kill, suspend or a
   USB Ethernet disconnect. Keep the other boards and computer connected.
6. Reacquire wired management without assuming a factory address. Capture the
   first observable new boot UUID and independent recovery progress. Allow the
   owner its separate reboot/readiness deadlines; it may request another clean
   vendor reboot. A response before a changed epoch is not proof of reboot.
7. Require `Restored`, changed epoch, all original vendor hashes and modes,
   empty owned UCI deltas, UP/carrier, expected original role/channel and successful
   Morse health. Verify that unrelated network/firewall/DHCP files and original
   LED/button services are unchanged. Check app identity/grants if an existing
   application was present. Late confirmation must be refused.
8. Privately archive the recovery guard/journals and raw evidence. Publish only
   public hashes, candidates, boot UUIDs, checkpoint/operator observations and
   outcome. Remove the isolated service/root only after operational restoration.
   Recheck wired management and the computer's independent Internet route.

If the 90-second window expires, report a missed checkpoint and let recovery
finish. Do not call it a passing electrical trial. If management does not return,
retain the backup/guard and investigate the wired path and owner state; do not
erase the journal, reenroll controllers or blindly replay mutation.

This first cut proves recovery from electrical loss between two synced vendor
file publications on that inspected device/image. It does not prove every flash
sector failure, a cut during a filesystem write, manager bootstrap/upgrades,
application staging or long-term flash wear. Later electrical trials should
cover application staging/launch accounting, radio guard replacement and
restoration publication. Each needs its own explicit checkpoint and evidence.
