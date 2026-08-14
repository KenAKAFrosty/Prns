#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

python3 validation/run.py verify
./tools/prns verify

while IFS=$'\t' read -r manifest packages; do
  echo "[fmt] ${manifest}"
  args=(--manifest-path "${manifest}")
  if [[ -n "${packages}" ]]; then
    IFS=',' read -ra selected <<< "${packages}"
    for package in "${selected[@]}"; do
      args+=(--package "${package}")
    done
  else
    args+=(--all)
  fi
  cargo fmt "${args[@]}" -- --check
done < <(
  python3 -c \
    'from validation.run import MANIFEST_PATH, load_toml; registry=load_toml(MANIFEST_PATH)["registry"]; overrides=registry.get("format_package_overrides", {}); print(*(f"{manifest}\t{chr(44).join(overrides.get(manifest, []))}" for manifest in registry["format_manifests"]), sep="\n")' \
    | tr -d '\r'
)

echo "[docs] intra-doc links (personal-rns)"
cargo doc --locked -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
