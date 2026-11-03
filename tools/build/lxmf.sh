#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repository_root}"

authority_python_version="3.13.7"
bootstrap_python="${LXMF_BOOTSTRAP_PYTHON:-python3}"
lxmf_venv="${LXMF_VENV:-${repository_root}/target/interop/lxmf-1.1.0}"
requirements="${repository_root}/prns-lxmf/wire/interop/python/requirements-lxmf-1.1.0.txt"
vector_generator="${repository_root}/prns-lxmf/wire/interop/python/generate_lxmf_1_1_0_vectors.py"
vector_tests="${repository_root}/prns-lxmf/wire/interop/python"

if [[ ! -x "${lxmf_venv}/bin/python" ]]; then
  if command -v uv >/dev/null 2>&1; then
    uv venv --python "${authority_python_version}" "${lxmf_venv}"
  else
    "${bootstrap_python}" -c \
      "import platform; assert platform.python_version() == '${authority_python_version}', platform.python_version()"
    "${bootstrap_python}" -m venv "${lxmf_venv}"
  fi
fi

"${lxmf_venv}/bin/python" -c \
  "import platform; assert platform.python_version() == '${authority_python_version}', platform.python_version()"

if command -v uv >/dev/null 2>&1; then
  uv pip sync --python "${lxmf_venv}/bin/python" "${requirements}"
else
  "${lxmf_venv}/bin/python" -m pip install \
    --disable-pip-version-check \
    --requirement "${requirements}"
fi

"${bootstrap_python}" -m unittest tools.tests.test_lxmf_publication_contracts

cargo fmt --manifest-path Cargo.toml -p prns-lxmf-wire -- --check

cargo check --locked \
  --manifest-path prns-lxmf/wire/Cargo.toml \
  --no-default-features \
  --target thumbv7em-none-eabihf
cargo test --locked \
  --manifest-path prns-lxmf/wire/Cargo.toml
cargo clippy --locked \
  --manifest-path prns-lxmf/wire/Cargo.toml \
  --all-targets \
  --no-default-features \
  -- \
  -D warnings

"${lxmf_venv}/bin/python" "${vector_generator}" --check
"${lxmf_venv}/bin/python" -m unittest discover \
  --start-directory "${vector_tests}" \
  --pattern 'test_*.py' \
  --verbose
echo "PRNS_LXMF_WIRE_GATE_OK"
