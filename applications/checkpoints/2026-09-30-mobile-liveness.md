# Mobile Bluetooth liveness: build and device checkpoint

The negotiated session-liveness implementation is integrated into `prns-app`.
Its purpose is to retire an unresponsive settled Bluetooth connection so normal
discovery can replace it, without app-level Bluetooth resets or evicting healthy
connections. Older peers retain their existing behavior. See the
[shared recovery notes](../../docs/bluetooth-session-recovery.md) for ownership,
negotiation, deadline rules, automated tests and firmware resource receipts.

## Final phone builds

Both standalone, locally signed Release builds use source
`d2d95831f231d20c4c47deecb0e09c26482d478e`. Subsequent integration is documentation
only. JavaScript is bundled; Metro is not required. These are development builds,
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

## Device observations and limit

Both apps were updated in place without uninstalling or clearing data. Galaxy
S9+ launched the final build at approximately 15:28 UTC and showed Running and
Bluetooth Ready, with unchanged primary/controller identities. Its displayed
conversation counts remained 23 and three (the bounded loaded history, not a
whole-database audit). MetalbeardMobile accepted the update, but iOS rejected its
first launch because the phone was locked. Mirroring had also timed out. Unlocking and
opening the updated iPhone app, then reconnecting Mirroring, remains necessary.

An earlier Android build from `7238ec907` was installed during the firmware checks.
Its primary/controller identities and displayed conversation counts were
unchanged, and it remained Running through a successful interval save. Those
observations belong to that interim binary, not the final artifact above. No
message was sent during that interim check.

No final-build two-phone delivery or restart-recovery pass is claimed yet. The
[September 29 checkpoint](2026-09-29-mobile-persistence-recovery.md) remains the
last physical messaging evidence: baseline delivery passed, but restarting only
iOS required a diagnostic Bluetooth reset on Android.

Private build/install logs are retained under
`/Volumes/wavlink/dev/prns-mobile-build/logs/`; the local observation ledger is
`scratch/prns-app/2026-09-30/phone-liveness-observations.json`. No board was flashed
for this continuation. Changes are committed locally; this continuation did not
push branches or open/update PRs.

## Next acceptance

1. Open the installed iPhone build and verify retained identities, contacts,
   pairings and conversations on both phones. Confirm both remain Running through
   an interval save.
2. Exchange BLE-only messages using saved contacts. Check delivery proof,
   receiver Verified source, matching IDs and duplicate counts separately.
3. Restart only iOS, leaving Android untouched. Measure automatic reconnection
   and saved-contact delivery without manual announce, Bluetooth reset, peer
   restart or another transport. Repeat with only Android restarted.

Record unsuccessful early attempts and elapsed recovery time, not just the final
retry. App Off/On, radio/permission recovery, long idle and natural suspension
remain separate acceptance cases. Cooperative close on normal shutdown also
remains follow-up implementation work.
