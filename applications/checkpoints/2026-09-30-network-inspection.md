# Network inspection acceptance on both phones

This records the September 30, 2026 Network slice and its focused standalone
Android/iOS acceptance. It was moved from the validation guide without rerunning
the checks. Source revisions, artifact hashes and boundaries below identify
exactly what was observed; newer source does not inherit these results.

## Source checks

The app-only Network slice separates physical Bluetooth connections, current
routes and a 200-entry accepted-announcement history. It uses existing public
inspection/observer APIs and regenerated TypeScript, Swift and Kotlin bindings;
the canonical Host contract is unchanged. Clear is generation-bound and does
not mutate contacts, routes or messages. Times describe the last native network
update, and current routes are not historical message-path evidence.

The isolated source checkpoint and integrated `prns-app` checks pass:

- 212 native host-test unit tests; the integrated Android-feature host run passes
  211 unit tests and all four integration tests. Strict native Clippy and
  formatting also pass. Regressions cover bounded retention, observer delegation,
  concurrent clear/admission, generation changes and failed route inspection.
- 430 app tests and 67 app-platform tests, including clear acknowledgement across
  navigation/refresh failure, newer observations, Stop and exact-width integers.
- Both TypeScript, formatting and lint gates, plus app routes, configuration and
  version checks. Product binding generation and its clean-output check pass,
  as does the app's web export (without a native runtime).

### Standalone phone builds and focused acceptance

MetalbeardMobile and the Galaxy S9+ were upgraded in place, without uninstalling
or clearing app data. The initial builds from `aac7b70db` exposed a tab-width
defect: the word “Announcements” broke across lines. Commit `6ad5efadb` sizes tabs
to their contents and wraps complete controls when needed, retaining OS text
scaling and the existing large-text stacked layout. All 434 app tests and app
TypeScript checks pass; the 24 Network screen tests include layout regressions.

Both final standalone Release builds use clean source
`6ad5efadbcb1a280b033997601b6a082d55a2b9f` and bundled JavaScript, not Metro.
Android remains API 29 minimum, with the single aggregate native library, valid
local development signature and 16 KB alignment; its eight native app unit tests
pass. iOS passes strict signature verification and retains its two Bluetooth
background modes and restoration identifiers, without AccessorySetupKit or a
development TCP fixture. These are locally signed development builds, not
production-distribution qualification.

| Final artifact | SHA256 |
| --- | --- |
| Android APK | `8cc174706188bedd4efd1328b39996dcf31b2de4bceebd6b797bee6fe7e5b227` |
| Android bundled JavaScript | `95ffe8cac7a521e44b5ee72fc1754da3d11c366cf1412f532f2484535c448a44` |
| iOS executable | `b5643e0ac367c0cb3205cbfa6941177cfe482ce5e9c8a4ee048cc3ac3ee3aaea` |
| iOS aggregate framework | `e7df0c3f5c0c841f051381d3876608b7195ed1d12b56918d1c04637b8456522a` |
| iOS bundled JavaScript | `49326c5988f2d8eaeeeb2505aec7e6f0b16eaff9d3c3ea371d6e0c3328e4d85f` |

The focused foreground journey passed on both final builds:

- Network tabs are readable at each phone's current text setting. Connections,
  one-hop Bluetooth routes and heard announcements appear separately. Expanded
  identifiers and connection traffic were inspected during the initial build
  journey; traffic also increased on the final iPhone build after messaging.
- Each phone heard the other's explicit announcement. Clearing the transient
  history changed its count to zero without removing the route or saved contact.
  A new announcement then appeared on both phones; Android recorded it as entry
  two, rather than resurrecting the cleared entry.
- Android sent “Network Android final” at 20:58:56 UTC, with delivery reported in
  144 ms. iOS sent “Network iOS final” at 21:01:12 UTC, with delivery in 193 ms.
  Both receiver Inboxes displayed the expected content. Message-detail source
  verification and full message-ID comparison were not repeated in this slice.
- Both demo conversations ended at 41 displayed messages, including the initial
  build's one iPhone test message. Other displayed conversation counts remained
  three on Android and 13 on iOS. Saved demo contacts and the iPhone's paired
  board remained present. These are loaded-history checks, not a database audit.

No board was flashed, system peer pairing added, or alternate transport configured.
This does not extend the separate background, radio, restart or long-idle
qualification. Enlarged-text layouts have automated coverage, not a new physical
large-text trial. Conversation delivery details remain the next inspection
increment; no further broad transport testing is required before that feature work.
