# Signed-candidate acceptance records

From 0.3.8 until 1.0, use schema-7 automated acceptance as described in
[the acceptance guide](../README.md#pre-10-automated-release-gate). The record
binds the signed candidate to complete passing exact-source release-readiness
evidence and explicitly makes no physical-qualification claim.

For historical physical acceptance, place a completed `VERSION.json` here only after testing the exact public signed prerelease. Create
the initial all-`not-run` record with the signed candidate's qualification generator, follow
`../README.md`, remove every placeholder through real observations, and submit the evidence through
normal default-branch review. The protected evidence workflow accepts only the file whose name
matches its version input and only from the explicitly supplied full commit on the default branch.
It also requires the exact signed-candidate tester roster, full UTC completion timestamps at or
after the prerelease publication instant, and every reviewed evidence object in the immutable
qualification-evidence archive.

Do not add synthetic examples or partially passing release records to this directory. Failed and
exploratory results stay outside the release evidence package; this path contains only a complete
promotion candidate whose underlying reviewed bytes remain available for verification.
