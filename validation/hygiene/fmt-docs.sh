#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

python3 validation/run.py verify
./tools/prns verify

while IFS= read -r manifest; do
  echo "[fmt] ${manifest}"
  cargo fmt --manifest-path "${manifest}" --all -- --check
done < <(
  python3 -c \
    'from validation.run import MANIFEST_PATH, load_toml; print(*load_toml(MANIFEST_PATH)["registry"]["format_manifests"], sep="\n")' \
    | tr -d '\r'
)

echo "[docs] intra-doc links (personal-rns)"
cargo doc --locked -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
