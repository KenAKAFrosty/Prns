# Flasher acceptance record

The acceptance record is evidence for one exact signed candidate, not a release checklist. Copy
`template.json` to a working `acceptance.json` only after the candidate manifest has been signed.
Populate its candidate identity from the exact files under test:

```sh
sha256sum flash-manifest.json flash-manifest.json.minisig
```

Use `shasum -a 256` on macOS when `sha256sum` is unavailable. `version`, `channel`,
`source_commit`, and `signing_key_id` must exactly match the signed manifest. Placeholder values,
future dates, unknown fields, incomplete matrices, and non-passing results fail closed.

## Physical runs

`runs` contains exactly one result for every shipping board, surface (`web` or `cli`), and host OS
(`macos`, `windows`, or `linux`): 24 rows in total. Each row records:

- the exact OS version and architecture;
- the manifest display name, observed PCB revision, and a tester-assigned nonsecret hardware label;
- the exact candidate client and, for web runs, the current stable Chrome or Edge version;
- a named scenario-to-`pass` map, tester, ISO date, and redacted evidence reference;
- `evidence.redaction: "reviewed"`, asserting that logs, screenshots, and videos were checked for
  credentials, device serial numbers, local paths, and other private data.

Use `hardware_revision: "not-marked"` only when the physical board exposes no revision. Do not
guess a revision, place a USB serial number in `hardware_identity`, or use `unknown` as evidence.
An example ESP web row is:

```json
{
  "board": "heltec-v4",
  "surface": "web",
  "os": "macos",
  "architecture": "aarch64",
  "os_version": "macOS 15.5 (24F74)",
  "hardware_identity": "lab-heltec-01",
  "hardware_model": "Heltec LoRa 32 V4",
  "hardware_revision": "1.0",
  "client": { "name": "prns-web-flasher", "version": "RELEASE_VERSION" },
  "browser": { "name": "chrome", "version": "138.0.7204.101" },
  "scenarios": { "fresh-install": "pass" },
  "result": "pass",
  "tester": "TESTER_IDENTITY",
  "date": "YYYY-MM-DD",
  "evidence": {
    "reference": "PUBLIC_OR_RELEASE_CONTROLLED_EVIDENCE_REFERENCE",
    "redaction": "reviewed"
  }
}
```

The example intentionally shows only the record shape. The validator requires aggregate scenario
coverage across the three OS rows for each board/surface pair.

## Transport-aware scenarios

ESP runs cover fresh install/update, board selection, zero/one/multiple devices, sparse writes,
wrong-chip rejection, BOOT/RESET recovery, disconnect boundaries, corrupt artifacts, signature
rejection, reset failure, and post-flash boot. Additional requirements are derived from the signed
manifest and surface:

- Web: permission denial, navigation warning, and device MD5 mismatch.
- CLI: unavailable port and write-verification failure.
- Heltec/T-Beam: Preserve, Configure, and Clear.
- Targets sharing a chip identity: explicit same-chip board confirmation.

T-Echo uses a distinct UF2 contract. Its web route proves signed download verification, truthful
manual-copy behavior, missing-mount/copy-failure guidance, reboot guidance, and post-flash boot. It
must not claim browser-side mount detection, filesystem sync, or device-side verification. Its CLI
route proves zero/one/multiple mounts, copy/flush/sync failures, mount disappearance, bounded reboot
detection and timeout, and post-flash boot.

The validator rejects scenario names that do not apply to a board's signed transport and surface.
The authoritative names and complete required sets live beside the validation logic in
`scripts/validate-flasher-acceptance.py`.

## Browser fallbacks

Unsupported-browser checks belong in `browser_fallbacks`, not physical flash rows. Record exact,
passing Firefox checks on macOS, Windows, and Linux plus Safari on macOS. Each must show the
truthful CLI/UF2 fallback without a broken connect action and carry its own redacted evidence.

## Native installation smoke

`installation_smoke` contains exactly one result for each published CLI target triple. The host OS
and architecture must agree with that target, the CLI version must equal the candidate version,
and `scenarios` must contain both `"install": "pass"` and `"doctor": "pass"`.

Validate a completed record with:

```sh
python3 scripts/validate-flasher-acceptance.py \
  --acceptance acceptance.json \
  --manifest CANDIDATE/flash-manifest.json \
  --manifest-signature CANDIDATE/flash-manifest.json.minisig
```

This schema records evidence; it does not create it. Never mark an unperformed scenario as passed.
