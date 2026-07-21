# Flasher tester rosters

Before building the candidate that will be signed, create `VERSION.json` here from
`../roster-template.json`. It must name real trusted testers for macOS arm64, macOS x86_64, Linux
x86_64, Linux arm64, and Windows x86_64. Use public nonsecret identities such as `github:handle`,
not email addresses.

Every assignment confirms access to all four shipping boards, working cables, serial/mount
permissions, the correct stable Chromium browser, the native CLI target, and reviewed recovery
instructions. Do not claim readiness that has not been confirmed with that tester.

Validate the roster against the exact candidate source identity:

```sh
python3 scripts/validate-flasher-tester-roster.py \
  --roster release/acceptance/rosters/VERSION.json \
  --version VERSION
```

The release build requires this exact committed roster and carries it inside the signed candidate
as `qualification/tester-roster.json`. The signed candidate checksum inventory and manifest source
commit bind those roster bytes without creating an impossible Git-hash self-reference. No roster
is synthesized by the repository; missing real assignments are an intentional go/no-go blocker.
Final acceptance requires each physical, browser-fallback, and native-installer row to name the
tester assigned to that row's exact OS and architecture. An unlisted identity cannot satisfy the
matrix even when every scenario is marked passing.
