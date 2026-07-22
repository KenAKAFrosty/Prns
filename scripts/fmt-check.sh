#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 validation/run.py verify

while IFS= read -r manifest; do
  echo "[fmt] ${manifest}"
  if [[ "${manifest}" == benchmarks/external/*/interop/Cargo.toml ]]; then
    # These standalone adapters have path dependencies in ignored, pinned
    # upstream checkouts. Format the adapter package without rewriting vendor
    # sources that are patched at build time.
    package="$(MANIFEST="${manifest}" python3 -c \
      'import os, tomllib; print(tomllib.load(open(os.environ["MANIFEST"], "rb"))["package"]["name"])')"
    cargo fmt --manifest-path "${manifest}" --package "${package}" -- --check
  else
    cargo fmt --manifest-path "${manifest}" --all -- --check
  fi
done < <(
  python3 -c \
    'import tomllib; print(*tomllib.load(open("validation/manifest.toml", "rb"))["registry"]["format_manifests"], sep="\n")'
)

echo "[docs] intra-doc links (personal-rns)"
cargo doc --locked -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
