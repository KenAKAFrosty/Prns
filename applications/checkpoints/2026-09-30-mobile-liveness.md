# Mobile Bluetooth liveness: build and device checkpoint

The negotiated session-liveness implementation is integrated into `prns-app`.
Its purpose is to retire an unresponsive settled Bluetooth connection so normal
discovery can replace it, without app-level Bluetooth resets or evicting healthy
connections. Older peers retain their existing behavior. See the
[shared recovery notes](../../docs/bluetooth-session-recovery.md) for ownership,
negotiation, deadline rules, automated tests and firmware resource receipts.

## Initial phone builds

The initial standalone, locally signed Release builds use source
`d2d95831f231d20c4c47deecb0e09c26482d478e`. Later diagnostic builds are identified
below. JavaScript is bundled; Metro is not required. These are development builds,
not production-distribution qualification.

| Artifact | SHA256 |
| --- | --- |
| Android APK | `61abc5c7065d60837fb2e979877460d556e1c5b0b2f6225d00a0b53ed62e1a20` |
| iOS executable | `159581e09f45c9559909cb2d1723285fdff294a3769a20393fa49995f1aceb2a` |
| iOS aggregate framework | `12d7414b0022d3be561d82bc51e951788c80ef152b4ff13ec4b6f0091f7946f2` |
| iOS bundled JavaScript | `2d473b390357b923f992ce813aeb0b62527483d6522bc41a184d3982db05e17b` |

Android's supported build helper passed its package, bundled-JavaScript, native
image and alignment checks; minimum Android remains API 29. The iOS app passed
strict signature verification and retains central/peripheral background modes
and scoped restoration identifiers, without AccessorySetupKit metadata or direct
linkage. Repository formatting, documentation and validation-registry checks pass.

## Device observations

Both apps were updated in place without uninstalling or clearing data.
MetalbeardMobile initially rejected launch while locked; the user then opened
the app and restored Mirroring. Both phones retained their primary/controller
identities and saved demo contact associations. The iPhone's paired board also
remained present. Both final builds stayed Running through a successful interval
save. Before sending, the displayed conversation counts were 23 and three on
Android, and 23 and 13 on iOS (bounded loaded history, not a whole-database audit).

An earlier Android build from `7238ec907` was installed during the firmware checks.
Its primary/controller identities and displayed conversation counts were
unchanged, and it remained Running through a successful interval save. Those
observations belong to that interim binary, not the final artifact above. No
message was sent during that interim check.

### Messaging and isolated restarts

Only AutomaticBluetoothLe was configured. No manual announce, Bluetooth reset,
data clearing or alternate transport was used in these trials. All times are UTC
on September 30.

| Trial | Result |
| --- | --- |
| Android to iPhone, 16:51:54 | Delivered in 57 ms; receiver Verified source and matching message IDs. |
| iPhone to Android, 16:54:54 | Delivered in 210 ms; receiver Verified source and matching message IDs. |
| Restart only iOS, 16:56:10 | Android retired its old connection at 16:57:07.876 and accepted a replacement at 16:57:09.623. The first attempted saved-contact send at 16:57:21 delivered in 210 ms, with receiver Verified source and matching IDs. Android's process was unchanged. |
| Restart only Android, 16:59:37 | The new process subscribed as central at 16:59:47, but its first send at 17:02:19 could not find the contact and was not queued. A reverse send from the untouched iPhone at 17:07:31 failed with no route. At 17:08 the iPhone showed Bluetooth Ready, without a peer. |

This initial iOS-only restart improved on the
[September 29 failure](2026-09-29-mobile-persistence-recovery.md), but the Android
restart still failed. The successful trial's generic Android
`policy close` log is consistent with the liveness deadline; it does not identify
the exact Rust retirement cause. Candidate connection churn began before that
restart and must not be attributed entirely to the restart.

The failed Android-central connection exchanged a 57-byte control greeting, but
no 9-byte liveness notifications were observed. A payload-free Android diagnostic
was then built and installed in place from `9ca6b1bd2`; its APK SHA256 is
`3a7f13f4f5f1c5434c8759c327653669d3c48a4c4072cad6dcea88431a0bbaca`.
Production Kotlin compilation and all 75 Kotlin tests passed before the build.
The iPhone process remained unchanged.

At 17:11:21 and 17:12:08, Android reported that the optional e9 capability was
absent despite local support being enabled, so it admitted a legacy connection.
After an OS disconnect and connection to a newly observed address, a saved-contact
message at 17:13:30 delivered in 94 ms with receiver Verified source and matching
IDs. This confirms that legacy data can work, not that the failed isolated
restart recovered without intervention: the diagnostic update restarted Android.

Android's discovery result alone does not distinguish a stale restored iOS
service from an Android discovery cache. The source at that point retained
restored services without e9 indefinitely; the repair and retest follow below. Post-greeting
rejection remains a hypothesis for the misleading connection. Successful delivery
alone does not qualify liveness in both native roles.

The failed reverse message was retried at 17:20:02 and delivered in 97 ms, with
receiver Verified source and the original message ID on both phones. This was a
retry of the existing message, not a new logical message.

### Restored-service repair and successful retest

Apple now replaces a known legacy restored service at startup, after PoweredOn
and before admitting listeners. Complete current services retain their original
objects. Exact service/characteristic ownership fences delayed callbacks; live
owners defer replacement and malformed services fail startup without removal.
The repair does not change embedded behavior or firmware resource contracts.

FFI tests pass 174 with one existing hardware-only ignore, with and without
logging. Strict all-target Clippy, formatting and iOS target checks pass.
MetalbeardMobile was updated in place at 17:27 using source
`26c7d1ceade684c8e6c4722565cc53dc99e4dc93`; the standalone Release build and strict
signature verification passed. Android kept the diagnostic APK above.

| Updated iOS artifact | SHA256 |
| --- | --- |
| Executable | `24831fac2a6923eea64f036c953e93db64130fac037e493437e0682409194390` |
| Aggregate framework | `e398d3431cf356598dbb6ae1c393a7e25d642174cd84631609dc466dd0a8b4e0` |
| Bundled JavaScript | `2d473b390357b923f992ce813aeb0b62527483d6522bc41a184d3982db05e17b` |

Android immediately discovered and successfully read the six-byte capability
without an Android restart or Bluetooth toggle. The replacement link exchanged
recurring nine-byte controls. A saved-contact iOS message at 17:27:45 delivered
in 96 ms with matching IDs and receiver Verified source.

| Isolated restart | Replacement connection | First attempted saved-contact send |
| --- | --- | --- |
| Android, 17:28:52 | Subscribed 17:29:13.630, about 22 seconds after command start; capability supported and recurring controls. iOS process unchanged. | 17:30:32, delivered in 56 ms; matching IDs and receiver Verified source. |
| iOS, 17:31:30 | Subscribed 17:31:47.442, about 18 seconds after command start; capability supported and recurring controls. Android process unchanged. | 17:33:11, delivered in 60 ms; matching IDs and receiver Verified source. |

Both isolated restarts therefore passed this bounded BLE-only journey without
manual announces, Bluetooth resets, alternate transports or restarting the
other phone. Delivery was tested about 100 seconds after each restart, not at
the instant of reconnection. Early Android candidates closed and retried
automatically. Close controls were observed during recovery, so these trials do
not specifically establish silent-peer timeout expiry or a recovery-time guarantee.
The release iOS build did not log an exact restoration-callback trace.

Before the controlled restarts, at 17:28, a competing connection closed and the
existing link hit a control-write error (status 133). A replacement connected
automatically. The cause is not established; do not attribute it to the later
restart or claim churn-free/long-idle stability.

At the end, each demo conversation displayed 31 messages: the original 23 plus
eight new logical messages, including the successful retry of the same failed
message. Other displayed counts remained three on Android and 13 on iOS. No
duplicate was apparent in this bounded loaded history. Both final binaries
retained their primary/controller identities, contacts and the iPhone's paired
board. At 17:38–17:40 both showed Running, Bluetooth Connected, Persistent Yes,
Restored Yes and Last flush Interval.

Private build/install logs are retained under
`/Volumes/wavlink/dev/prns-mobile-build/logs/`; the local observation ledger is
`scratch/prns-app/2026-09-30/phone-liveness-observations.json`. No board was flashed
for this continuation. Changes are committed locally; this continuation did not
push branches or open/update PRs.

## Remaining acceptance

1. Exercise app Off/On and radio recovery independently, preserving saved records.
2. Qualify silent-peer expiry, long idle and natural suspension; capture enough
   native evidence to distinguish policy expiry from received Close or OS errors.
3. Investigate recurring connection churn if it reproduces during those trials.

Record unsuccessful early attempts and elapsed recovery time, not just the final
retry. Permission recovery also remains a separate case. Cooperative close on
normal shutdown remains follow-up implementation work.
