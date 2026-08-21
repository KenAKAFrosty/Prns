# Validation hub

This directory centralizes orchestration and evidence for cross-cutting
validation without relocating unit, property, or private proof bodies from their
owning code.

Start with:

```console
python3 validation/run.py verify
python3 validation/run.py list
```

(Use `python` instead of `python3` on Windows.)

`manifest.toml` is the executable inventory; `run.py` is the dependency-free
operator and CI interface. See `docs/validation.md` for tiers, evidence,
mutation-triage policy, stock-RNS setup, and exact-SHA release qualification.
The synchronized pre-change findings and their dispositions are preserved in
`BASELINE.md`.

Hardware-only release checks live beside their platform gates. The provisional
Windows BLE procedure is in
[`platforms/windows-ble-hardware.md`](platforms/windows-ble-hardware.md).
Hardware qualification receipts are collected under
[`qualifications/`](qualifications/).
The current T096 and T1000-E developer-flasher evidence is in
[`flasher-0.3.7-qualification.md`](qualifications/flasher-0.3.7-qualification.md).
The production mobile matrices are in
[`platforms/android-production-hardware.md`](platforms/android-production-hardware.md)
and
[`platforms/ios-production-hardware.md`](platforms/ios-production-hardware.md).
