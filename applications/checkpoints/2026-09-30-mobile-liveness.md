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

The iOS-only restart therefore improves on the
[September 29 failure](2026-09-29-mobile-persistence-recovery.md), but reciprocal
restart acceptance is **not complete**. The successful trial's generic Android
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
service from an Android discovery cache. The source does retain restored services
without e9 indefinitely; that upgrade path is being repaired. Post-greeting
rejection remains a hypothesis for the misleading connection. Successful delivery
alone does not qualify liveness in both native roles.

Private build/install logs are retained under
`/Volumes/wavlink/dev/prns-mobile-build/logs/`; the local observation ledger is
`scratch/prns-app/2026-09-30/phone-liveness-observations.json`. No board was flashed
for this continuation. Changes are committed locally; this continuation did not
push branches or open/update PRs.

## Next acceptance

1. Diagnose and repair the failed Android-central recovery without weakening
   established-session ownership or evicting a healthy connection on a greeting.
2. Repeat isolated restarts in both directions with capability negotiation
   observed, then verify saved-contact delivery, receiver Verified source,
   matching IDs and duplicate counts separately.

Record unsuccessful early attempts and elapsed recovery time, not just the final
retry. App Off/On, radio/permission recovery, long idle and natural suspension
remain separate acceptance cases. Cooperative close on normal shutdown also
remains follow-up implementation work.
