#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "${repository_root}"

authority_python_version="3.13.7"
bootstrap_python="${LXMF_BOOTSTRAP_PYTHON:-python3}"
lxmf_venv="${LXMF_VENV:-${repository_root}/applications/target/interop/lxmf-1.1.0}"
requirements="${repository_root}/applications/services/lxmf-wire/interop/python/requirements-lxmf-1.1.0.txt"
vector_generator="${repository_root}/applications/services/lxmf-wire/interop/python/generate_lxmf_1_1_0_vectors.py"
vector_tests="${repository_root}/applications/services/lxmf-wire/interop/python"
live_harness="${repository_root}/applications/services/lxmf/interop/python/run_live_tcp_lxmf.py"
live_fixture_tests="${repository_root}/applications/services/lxmf/interop/python"

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

cargo fmt --manifest-path applications/Cargo.toml --all -- --check

cargo check --locked \
  --manifest-path applications/services/lxmf-wire/Cargo.toml \
  --no-default-features \
  --target thumbv7em-none-eabihf
cargo test --locked \
  --manifest-path applications/services/lxmf-wire/Cargo.toml
cargo clippy --locked \
  --manifest-path applications/services/lxmf-wire/Cargo.toml \
  --all-targets \
  --no-default-features \
  -- \
  -D warnings

cargo check --locked \
  --manifest-path applications/services/lxmf/Cargo.toml \
  --no-default-features
cargo check --locked \
  --manifest-path applications/services/lxmf/Cargo.toml \
  --no-default-features \
  --target thumbv7em-none-eabihf
cargo test --locked \
  --manifest-path applications/services/lxmf/Cargo.toml \
  --features tokio-host
cargo clippy --locked \
  --manifest-path applications/services/lxmf/Cargo.toml \
  --all-targets \
  --features tokio-host \
  -- \
  -D warnings

"${lxmf_venv}/bin/python" "${vector_generator}" --check
"${lxmf_venv}/bin/python" -m unittest discover \
  --start-directory "${vector_tests}" \
  --pattern 'test_*.py' \
  --verbose
"${lxmf_venv}/bin/python" -m unittest discover \
  --start-directory "${live_fixture_tests}" \
  --pattern 'test_*.py' \
  --verbose
"${lxmf_venv}/bin/python" "${live_harness}"

echo "APPLICATION_LXMF_DIRECT_GATE_OK"
