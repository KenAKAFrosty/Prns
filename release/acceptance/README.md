# Flasher acceptance record

The acceptance record is evidence for one exact signed candidate, not a checklist or a place to
record intentions. Generate it only after the public prerelease exists. The generator binds the
manifest, manifest signature, signed-candidate archive, and signed roster by exact identity and
produces eighteen physical rows, three Firefox Web Serial rows, one unsupported-browser row, and
five native installer rows as `not-run`:

```sh
PUBLISHED_AT="$(gh release view vVERSION --json publishedAt --jq .publishedAt)"
python3 qualification/create-flasher-acceptance.py \
  --manifest CANDIDATE/flash-manifest.json \
  --manifest-signature CANDIDATE/flash-manifest.json.minisig \
  --signed-bundle prns-flasher-candidate-vVERSION-signed.tar.gz \
  --tester-roster CANDIDATE/qualification/tester-roster.json \
  --prerelease-published-at "$PUBLISHED_AT" \
  --output acceptance.json
```

The generator refuses to overwrite a record. Never mark an unperformed scenario as passed. The
validator rejects placeholders, future dates, unknown fields, incomplete matrices, non-passing
results, and any candidate identity that differs from those three exact files.

## Physical runs

`runs` contains one result for every board and surface (`web` or `cli`) plus separate S140 6.1.1
and 7.3.0 T-Echo results on both surfaces: eighteen rows. The pinned S140 6.1.1 variant for T114
and T096 contributes one row per surface. The signed roster assigns each
board/surface pair to one supported host, with Linux, macOS, and Windows
collectively represented on both surfaces. One person may hold multiple or all assignments; an
assignment is a coverage obligation, not a distinct-person requirement. Each row records:

- the exact OS version and architecture;
- the signed-manifest display name, observed PCB revision, and a tester-assigned nonsecret label;
- the exact client and, for web runs, the current stable Chrome or Edge version and `stable`
  channel;
- named scenario results, the exact signed-roster tester identity, a full UTC `completed_at`
  timestamp no earlier than the prerelease `publishedAt`, and immutable redacted evidence;
- its own passing fresh install, update, correct-board, post-flash-boot, and every applicable
  transport, provisioning, and recovery observation.

Use `hardware_revision: "not-marked"` only when the board exposes no revision. Never put a USB
serial number in `hardware_identity`. Every evidence object has this fail-closed form:

```json
{
  "reference": "artifact://qualification/LOWERCASE_EVIDENCE_SHA256",
  "sha256": "LOWERCASE_EVIDENCE_SHA256",
  "redaction": {
    "reviewer": "REVIEWER_IDENTITY",
    "credentials_removed": true,
    "device_identifiers_removed": true,
    "local_paths_removed": true,
    "private_network_data_removed": true
  }
}
```

The reference must be exactly `artifact://qualification/LOWERCASE_EVIDENCE_SHA256`. Place the exact
nonempty reviewed object at `EVIDENCE_ROOT/LOWERCASE_EVIDENCE_SHA256`; URLs and hash assertions are
not evidence. The validator reads the object and recomputes its digest, rejects missing, extra,
linked, misnamed, empty, or mismatched objects, and binds the resulting deterministic evidence
archive into the signed release record. The redaction reviewer must inspect each object after
collection. Do not treat automated substitution as review.

## Transport-aware scenarios

ESP runs cover install/update, board selection, zero/one/multiple devices, sparse writes,
wrong-chip rejection, BOOT/RESET recovery, disconnect boundaries, corrupt artifacts, signature
rejection, reset failure, and post-flash boot. Additional requirements derive from the signed
manifest and surface:

- Web: permission denial, navigation warning, and device MD5 mismatch.
- CLI: unavailable port and write-verification failure.
- Heltec/T-Beam: Preserve, Configure, and Clear.
- Targets sharing a chip: explicit same-chip board confirmation.

The T-Echo, T114, and T096 use the UF2 contract. The web route proves signed download verification, truthful
manual-copy behavior, missing-mount/copy-failure guidance, reboot guidance, and post-flash boot. It
must parse `INFO_UF2.TXT` locally, reject malformed or unsupported foundations, select only the
matching signed variant, and never upload or retain the descriptor. It must not claim browser-side
mount detection, filesystem sync, or device-side verification. Its CLI route proves exact
foundation detection, zero/one/multiple mounts, copy/flush/sync failures, mount disappearance,
bounded reboot detection and timeout, newly enumerated application USB identity, and post-flash
boot. Each compatibility row also proves the interfaces declared by that board. Evidence bytes may
not be reused between the T-Echo S140 6.1.1 and 7.3.0 rows or between distinct boards.

T-1000E uses the Nordic serial-DFU contract. Both surfaces prove the exact signed application,
init packet, and manifest-bound recovery UF2; exact application/bootloader identity; reliable
transfer and activation; recovery guidance; LoRa and USB operation; and post-flash boot. The web
row additionally proves managed-application WebUSB entry, exact Web Serial bootloader selection,
permission denial, and navigation protection. The CLI row proves zero/one/multiple-device
handling, port failure, bootloader timeout, bounded transfer retry, and non-writing doctor output.

The authoritative scenario sets and roster-derived rows live in
`qualification/flasher_acceptance_contract.py`, used by both generator and validator.

## Firefox Web Serial smoke

`web_serial_smoke` contains one hardware-backed stable Firefox result on each of macOS, Windows,
and Linux. Each row uses the eligible shipping ESP-serial board and host from the signed roster,
records the exact OS, architecture, hardware identity/model/revision, Firefox and flasher versions,
tester, completion timestamp, and immutable evidence, and must pass exactly five scenarios:
permission grant, one-device selection, correct-board selection, fresh signed-candidate install,
and post-flash boot. UF2 boards and unsupported-page observations do not satisfy these smokes. Each
OS row requires distinct evidence.

## Browser fallback

`browser_fallbacks` records stable Safari on macOS. Every row must prove all six points: the ESP
CLI guidance is present, ESP connect is unavailable, no broken connect action is shown, and the
T-Echo and T096 UF2 routes and T-1000E recovery-UF2 route remain available. The fallback check is
separate from successful Web Serial flashing.

## Native installation smoke

`installation_smoke` contains exactly one result for each published CLI target triple. The host OS
and architecture must agree with the target. Each row proves the exact public archive installs and
that `hopspot-flash --version` reports the exact candidate version, so both `install` and `version`
must pass. These rows may run on matching hosted runners and do not require a board. They do not
replace the board-backed CLI assignments.

Validate the completed record with the same exact inputs:

```sh
python3 qualification/validate-flasher-acceptance.py \
  --acceptance acceptance.json \
  --manifest CANDIDATE/flash-manifest.json \
  --manifest-signature CANDIDATE/flash-manifest.json.minisig \
  --signed-bundle prns-flasher-candidate-vVERSION-signed.tar.gz \
  --tester-roster CANDIDATE/qualification/tester-roster.json \
  --evidence-root EVIDENCE_ROOT \
  --prerelease-published-at "$PUBLISHED_AT"
```

Package `EVIDENCE_ROOT` with `qualification/package-flasher-qualification-evidence.py` as
`qualification-evidence-vVERSION.tar.gz`. Commit only a complete passing record to
`release/acceptance/records/VERSION.json` through normal review. Failed and exploratory
observations stay outside the release evidence package.
