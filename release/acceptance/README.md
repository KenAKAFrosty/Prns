# Flasher acceptance record

Copy `template.json` into the signed candidate as `acceptance.json`. Each `runs` item records:

- `board`: one shipping board slug
- `surface`: `web` or `cli`
- `os`: `macos`, `windows`, or `linux`
- `architecture`: `aarch64` or `x86_64` as applicable
- `hardware_identity`, `client_version`, `tester`, `date`, and `result: "pass"`
- `browser`: stable Chrome plus version on macOS/Linux, or Edge plus version on Windows
- `scenarios`: an object mapping every exercised scenario name to `"pass"`

There must be one passing run for every board/surface/OS combination. Across those runs, record
physical flashing on every published architecture and cover the common, provisioning, and T-Echo
failure/recovery scenarios enforced by `scripts/validate-flasher-acceptance.py`. Each web run also
records the truthful Firefox-to-CLI fallback; macOS web runs additionally record the Safari-to-CLI
fallback. Device disconnects are exercised before the first write, during a part write, and after
verification but before reset.

Each `installation_smoke` item records `target`, `cli_version`, `tester`, `date`, and
`result: "pass"`. All five published CLI target triples are required. The protected promotion
workflow rejects incomplete, stale, or non-passing evidence.
