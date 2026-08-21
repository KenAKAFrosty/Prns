# Exact signed-candidate flasher qualification

These steps test the public prerelease bytes. An unsigned build, a rebuilt website, or a candidate
whose archive digest differs from the public release does not count.

## 1. Establish the candidate identity

Download these assets from the same `vVERSION` GitHub prerelease:

- `prns-flasher-candidate-vVERSION-signed.tar.gz`
- `prns-flasher-attestation-vVERSION.json`
- `prns-flasher-attestation-vVERSION.metadata.json`
- `flash-manifest.json` and `flash-manifest.json.minisig`
- `SHA256SUMS.txt`, `SHA256SUMS.txt.minisig`, and `minisign.pub`

Record the signed archive SHA-256 shown in the prerelease notes and require it to match the file.
Record the exact GitHub `publishedAt` value; every counted observation must use a full UTC
`completed_at` timestamp at or after that instant:

```sh
PUBLISHED_AT="$(gh release view vVERSION --json publishedAt --jq .publishedAt)"
```

With GitHub CLI installed, independently verify its provenance:

```sh
gh attestation verify prns-flasher-candidate-vVERSION-signed.tar.gz \
  --repo KenAKAFrosty/Prns \
  --bundle prns-flasher-attestation-vVERSION.json \
  --signer-workflow KenAKAFrosty/Prns/.github/workflows/flasher-sign.yml \
  --deny-self-hosted-runners
```

Extract the archive to a new directory named `CANDIDATE`. Verify the signed checksum document,
then every file it names:

```sh
minisign -Vm CANDIDATE/SHA256SUMS.txt \
  -x CANDIDATE/SHA256SUMS.txt.minisig \
  -p CANDIDATE/minisign.pub
python3 CANDIDATE/qualification/verify-flasher-candidate-files.py CANDIDATE
```

The Python verifier is platform-neutral and rejects missing, extra, traversing, symlinked, or
tampered payloads. Stop if any signature, hash, file size, source commit, version, or key ID
differs.

## 2. Web qualification

Use the server shipped inside the verified candidate. It binds only to loopback, sends `no-store`,
and performs SPA fallback only for extensionless routes; missing firmware or JavaScript remains a
real 404.

```sh
python3 CANDIDATE/qualification/serve-flasher-candidate.py \
  --website CANDIDATE/website \
  --port 8000
```

Open `http://localhost:8000/flash`. Use current stable Chrome on macOS/Linux and current stable Edge
on Windows. Keep the terminal server running until the scenario ends. The candidate is entirely
local after extraction; do not replace its manifest, firmware, website, or flasher bundle.

Test the assigned board/OS scenarios from `acceptance.json`. Each physical row must independently
show a fresh install and expected post-flash boot. Preserve is the configuration default. For
Heltec and T-Beam, explicitly confirm the board image/name because their shared ESP32-S3 identity
cannot distinguish the products. Perform the three roster-assigned Firefox Web Serial smokes on
macOS, Windows, and Linux with physical ESP-serial boards. Each smoke must show permission grant,
one-device selection, correct-board selection, a fresh install from the exact signed candidate,
and post-flash boot, with distinct immutable evidence for every OS. Record only Safari as the
unsupported-browser fallback.

## 3. CLI qualification

Install the exact checked archive for the host from `CANDIDATE/cli`, then import the extracted
candidate into the immutable verified cache:

```sh
hopspot-flash cache import CANDIDATE
```

Read `VERSION` and the manifest release channel, then use both explicitly for every qualification
flash:

```sh
hopspot-flash flash BOARD \
  --channel preview \
  --version VERSION \
  --offline \
  --yes \
  --wifi preserve
```

Replace `preview` only if the signed manifest says `stable`. `--offline` is mandatory for counted
CLI qualification. Use the masked guided entry or `--wifi-password-stdin` for Configure; never put
a password on the command line. T-Echo, T114, and T096 stay on the signed UF2 mount/copy route and
must resolve their exact pinned variant from the mounted bootloader identity before reading it from
the verified cache. T-1000E uses the exact Nordic serial-DFU application and init packet; its
manifest-bound recovery UF2 is the fallback when the serial bootloader cannot be entered.

Run `hopspot-flash doctor BOARD` as part of each physical CLI assignment. On ESP boards it opens a
non-writing identity session; on UF2 boards it reports the Board-ID, bootloader version,
SoftDevice, and exact compatibility variant without writing; on T-1000E it reports the exact
application or bootloader mode without writing. Heltec versus T-Beam remains a same-chip limitation
and must be confirmed by the tester.

The five native installation rows are separate archive checks. Each one runs on its target OS and
architecture, installs the exact public archive, and confirms that `hopspot-flash --version` reports
the candidate version. Matching hosted runners may cover target architectures without local
hardware. These rows never substitute for the physical CLI assignments.

After the prerelease is public, dispatch `flasher-installation-qualification.yml` from the exact
candidate commit on the protected default branch with the version, source commit, and independently
recorded signed-candidate SHA-256. Its five target-matched jobs re-fetch the public assets, verify
the signed checksums and attestations, run the checked installer, and upload one roster-bound JSON
evidence object per target. The assigned tester reviews those objects before their exact bytes are
hashed into the qualification evidence store.

## 4. Capture evidence without secrets

Never record Wi-Fi credentials, USB serial numbers, local user paths, private network details,
tokens, or signing material. After collection, a named reviewer redacts each evidence object and
computes the SHA-256 of its exact reviewed bytes. Store that object in a flat `EVIDENCE_ROOT`
directory under its lowercase SHA-256 filename. The acceptance reference is exactly
`artifact://qualification/THAT_SHA256`; URLs and externally asserted hashes do not count. The
tester then fills only scenarios actually observed, names the tester assigned to that exact
coverage row in `CANDIDATE/qualification/tester-roster.json`, and records a full UTC
`completed_at` value.

If a flash is interrupted mid-part, record the failure, disconnect cleanly, follow the displayed
BOOT/RESET recovery, and restart the entire sparse plan. Do not represent an unsafe resume.

## 5. Validate the record

The release owner creates the initial record from the exact public files:

```sh
python3 CANDIDATE/qualification/create-flasher-acceptance.py \
  --manifest CANDIDATE/flash-manifest.json \
  --manifest-signature CANDIDATE/flash-manifest.json.minisig \
  --signed-bundle prns-flasher-candidate-vVERSION-signed.tar.gz \
  --tester-roster CANDIDATE/qualification/tester-roster.json \
  --prerelease-published-at "$PUBLISHED_AT" \
  --output acceptance.json
```

After all assignments are complete, package the reviewed objects deterministically and upload that
exact archive to the same prerelease without replacing it:

```sh
python3 CANDIDATE/qualification/package-flasher-qualification-evidence.py \
  EVIDENCE_ROOT qualification-evidence-vVERSION.tar.gz
gh release upload vVERSION qualification-evidence-vVERSION.tar.gz
```

Retain the printed archive SHA-256 for the protected finalization dispatch. Finalization downloads
that fixed asset, checks the operator-supplied archive hash, extracts it without links or escaping
paths, and hashes every referenced object before signing acceptance. Validate locally against the
same bytes:

```sh
python3 CANDIDATE/qualification/validate-flasher-acceptance.py \
  --acceptance acceptance.json \
  --manifest CANDIDATE/flash-manifest.json \
  --manifest-signature CANDIDATE/flash-manifest.json.minisig \
  --signed-bundle prns-flasher-candidate-vVERSION-signed.tar.gz \
  --tester-roster CANDIDATE/qualification/tester-roster.json \
  --evidence-root EVIDENCE_ROOT \
  --prerelease-published-at "$PUBLISHED_AT"
```

The generator starts every result as `not-run`, and the validator accepts only a complete passing
matrix tied to that exact signed archive, signed roster, prerelease publication instant, and
locally verified evidence bytes.
