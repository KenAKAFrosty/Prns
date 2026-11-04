# RemoteControl pairing maintainer checks

Record the exact source commit, firmware checksum, controller/app version,
physical board revision, host/phone OS and reset method for every result.
Build success does not establish physical qualification. Keep USB enrollment,
menu pairing, radio configuration and over-air group delivery as separate results.

## Qualification scope

Test the exact firmware and controller builds intended for distribution. A
working-tree trial, firmware build or successful upgrade with an existing grant
does not establish new invitation pairing, full power-cycle recovery or another
board model's behavior. Keep redacted results and checksums in CI/release artifacts
or ignored validation output, rather than adding device trial logs to this guide.

## Board matrix

| Board | Pairing path | Flash | Physical checks that matter |
| --- | --- | --- | --- |
| Heltec Vision Master E290-HF | Shared menu; user button GPIO21 | 16 MB | Normal menu font/alignment, e-ink refresh, configured and unconfigured startup |
| Heltec WiFi LoRa 32 V3/V3.1 | Shared menu on upstream S3FN8 runtime | 8 MB | Button actions, confirmation ordering, repeated sessions, SRAM stability |
| Heltec LoRa 32 V4/S3R2 | Shared menu | 16 MB | Display/button behavior, restart access, optional radio check |
| Heltec LoRa 32 V4 R8/S3R8 | Shared menu | 16 MB | Correct R8 firmware, display/button behavior, restart access |
| LilyGO T-Beam S3 Supreme/SX1262 | Shared menu | 8 MB | Display/button behavior, OLED recovery after reset/power cycle |

Confirm the physical model, radio band, flash size and existing partition layout.
ESP32-S3 detection alone cannot distinguish these boards. Do not interchange V4
and V4 R8 images. This menu matrix does not include Tracker V2 or nRF boards.

## Board controls and peripheral checks

On V3/V3.1, V4, V4 R8 and T-Beam Supreme, use the BOOT/user button on GPIO0,
not Reset or the Supreme's PMU/power button. Short presses move the selection;
holding activates it. Release between holds and release BOOT before reset or
power-up so the board starts normally instead of entering its ROM downloader.
The E290 uses its GPIO21 user button.

Check that the invitation, confirmation code and both action rows fit the display.
Exercise Screen Off and wake after pairing. On Supreme, distinguish an OLED
recovery failure from lost controller access, and record USB/battery conditions:
unplugging USB while a battery remains connected is not a full power cycle.
Optional V4/Supreme GNSS must not be required for pairing. If fitted, check GPS
On/Off without losing menu control or authenticated access; record receiver and
fix results separately.

## Preserve existing installations

Use the canonical board package and partition table. Back up all flash privately
before changing firmware. Backups can contain secret identities and grants;
never attach them to a public report. Close other serial users first. A backup
must finish successfully and have the full expected byte length.

With espflash 4.7.0, keep its RAM stub enabled. For slow/unreliable reads, use
`--block-size 0x1000 --max-in-flight 1`. If reading separate chunks, keep the
application stopped, use consecutive addresses and compare the assembled file's
MD5 with the whole-device checksum before treating it as a usable backup.

Flash only the supplied bootloader, partition table and application regions.
Do not erase the whole chip or write a filled image across retained partitions.
A compatible partition layout is required; unknown layouts need a separate
migration. Disable any flash-time enrollment option when updating an existing
board. Preserve its identity even when another controller will be added.

## E290 invitation and recovery checks

Keep the existing recovery administrator. Use a new independent controller for
this lane so prior authorization cannot hide a failed pairing attempt.

1. Complete app setup without a board, then choose to add one later.
2. Open **Menu → Pairing** on the board. Check the font, alignment and invitation.
3. Select the board and enter its invitation in the controller. Compare the
   confirmation code on both screens and review the requested authority.
4. Approve on the board and controller. Both must reach durable success. A
   controller-only approval must not imply that the board approved.
5. Keep existing radio settings or choose to finish configuration later. Perform
   an authenticated read. A failed configuration/read must retain saved access.
6. Restart the app and fully power-cycle the board. Reconnect and authenticate
   without pairing again. Record startup time and any retained-state warning.
7. Check a refused/mismatched confirmation, expired invitation, board/controller
   cancellation and interrupted confirmation using an unpaired controller.
   None may report successful authorization without durable completion. Rejecting
   a new attempt must not revoke an older grant. Recover uncertain outcomes by
   checking state, not by automatically replaying authorization.
8. Reopen pairing after each failed/cancelled attempt. On V3, repeat open/cancel
   ten times and make five sequential authenticated reads to check stability.

Also test a missing second approval with the independent controller: approve
only on the board, wait beyond the 60-second attempt deadline, and confirm no
Paired result or usable new authorization. Reopen pairing and complete a fresh
attempt. Preserve leading zeroes in the eight-character invitation; the initial
invitation window is about 120 seconds. A board approval screen or queued
announcement alone does not establish a saved grant.

After negative checks, perform an authenticated read with the original controller.
If the new client can check target-side authorization, verify denial there too;
a missing saved-target entry alone does not establish target-side denial.
For a bounded stability pass, keep the board connected for at least 15 minutes,
use menus and occasional authenticated reads, and record resets or growing delays.
Keep an already configured radio's ordinary traffic unchanged and report it;
this bounded check does not establish other traffic loads or indefinite uptime.

An unconfigured LoRa card should show **SubG / Setup**, with no Failed state.
Pairing alone must not choose a region/profile or start receiving/transmitting.
A configured board must keep its profile. A separate deliberate radio test needs
compatible settings and antennas; Bluetooth pairing success does not prove LoRa.

## USB setup and independent controllers

- Export a controller card or raw public key, enroll over the physical USB
  connection and verify the pinned target. Unsupported firmware must fail before
  an authorization request.
- Export/import target access only after the matching Saved receipt. Reject a
  card addressed to another controller and conflicting existing access.
- Check waiting, Continue checking, Stop checking, disconnect/reconnect and stale
  receipts. Reconciliation must not submit another grant.
- Authenticate with imported access, restart the controller and board, and repeat.
- Delegate a narrower grant between distinct controllers. Verify permitted reads,
  denied administrator operations and exact permission retention after restart.
- Record third-party controller builds precisely. Raw-key compatibility does not
  establish support for the public-card carrier.

## Result template

| Check | Pass / fail / skipped | Evidence or reason |
| --- | --- | --- |
| Exact firmware and controller identities recorded | | |
| Backup and compatible retained layout verified | | |
| Normal startup, menu font and button actions | | |
| New invitation and matching code | | |
| Approval required on both devices | | |
| Authenticated access after durable pairing | | |
| Existing radio settings kept / configuration deferred | | |
| App restart and full board power cycle | | |
| Refusal, mismatch, expiry and cancellation | | |
| Interrupted confirmation and recovery | | |
| Existing grants remain usable | | |
| USB setup and receipt reconciliation, if tested | | |
| Repeated sessions / stability | | |

Attach redacted logs or observations with timings. Do not include private keys,
flash backups, controller-state directories, Wi-Fi credentials, active invitation
codes or complete device inventories. Group delivery is a separate qualification
and is not required to demonstrate the shared pairing flow.
