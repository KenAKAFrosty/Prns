# Flasher tester rosters

Before building the candidate that will be signed, create `VERSION.json` here from
`../roster-template.json`. It must contain sixteen physical board/surface assignments, three Firefox
Web Serial assignments, one Safari fallback assignment, and five published-archive installer
assignments. The physical assignments collectively cover Linux, macOS, and Windows on both
surfaces. Use public nonsecret identities such as `github:handle`, not email addresses.

The same person may hold multiple or all assignments. The roster models required coverage, not a
minimum team size. Every physical assignment confirms access to its named board, working cables,
serial/mount permissions, the correct stable Chromium browser when applicable, and reviewed
recovery instructions. Each Firefox Web Serial assignment binds one eligible shipping ESP-serial
board to its required desktop OS and exact stable browser. Safari fallback and installer
assignments separately confirm access to their exact browser or target archive. Do not claim
readiness that has not been confirmed.

Validate the roster against the exact candidate source identity:

```sh
./tools/prns release tester-roster validate -- \
  --roster release/acceptance/rosters/VERSION.json \
  --version VERSION
```

The release build requires this exact committed roster and carries it inside the signed candidate
as `qualification/tester-roster.json`. The signed candidate checksum inventory and manifest source
commit bind those roster bytes without creating an impossible Git-hash self-reference. No roster
is synthesized by the repository; missing real assignments are an intentional go/no-go blocker.
Final acceptance requires each physical, Firefox Web Serial, Safari fallback, and native-installer
row to match its exact assignment. An unlisted identity, substituted board, or substituted host
cannot satisfy the matrix even when every scenario is marked passing.
