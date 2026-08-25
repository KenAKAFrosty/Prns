# Flasher hotfix specifications

A file named `SUITE_VERSION-hotfix.N.json` authorizes one immutable, target-scoped flasher release.
The repository `VERSION` remains the suite version. The specification pins the exact current stable
base and lists every board whose firmware must be rebuilt.

Dispatch `flasher-candidate.yml` on the protected default branch with:

```text
channel=stable
history_mode=retain
history_version=BASE_VERSION
history_release_record_sha256=BASE_RELEASE_RECORD_SHA256
hotfix_version=SUITE_VERSION-hotfix.N
```

The base may be the suite release or an earlier numbered hotfix in the same suite. Candidate
construction builds only `changed_boards`, inherits every other shipping target byte-for-byte, and
writes `metadata/hotfix.json`. Validation rejects altered inherited bytes, a mismatched history
head, an uncommitted specification, or a declaration that rebuilds every shipping board.

`qualification.physical_boards` and `qualification.deferred_hardware` must be disjoint and exactly
partition `changed_boards`. Physical boards get one schema-6 row for each listed surface and must
pass exactly the committed scenarios and checks against the public signed candidate. Each deferred
board records a specific technical basis and a concrete follow-up. The release owner approves that
deferral after prerelease publication; no physical result or evidence may be invented for it.

After targeted acceptance, the existing signing, evidence finalization, rollback dry-run, and
promotion workflows are used unchanged with the hotfix version. Never replace the base release or
reuse a hotfix version.
