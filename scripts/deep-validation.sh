#!/usr/bin/env bash
# Operator lane for the expensive validation surface. Normal CI runs the cheap
# guards; this script is the one-command path for proof, fuzz, interop, and
# mutation sanity when hardening a release or architecture change.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "${repo_root}"

mode="full"
fuzz_seconds="${PRNS_DEEP_FUZZ_SECONDS:-30}"
run_mutants="${PRNS_DEEP_MUTANTS:-0}"
run_android="${PRNS_DEEP_ANDROID:-0}"
run_interop="${PRNS_DEEP_INTEROP:-1}"
artifact_dir="${PRNS_VALIDATION_ARTIFACTS:-validation-artifacts}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --quick)
      mode="quick"
      fuzz_seconds="0"
      run_interop="0"
      ;;
    --full)
      mode="full"
      ;;
    --mutants)
      run_mutants="1"
      ;;
    --android)
      run_android="1"
      ;;
    --no-interop)
      run_interop="0"
      ;;
    --fuzz-seconds)
      shift
      fuzz_seconds="${1:?missing seconds after --fuzz-seconds}"
      ;;
    *)
      echo "usage: scripts/deep-validation.sh [--quick|--full] [--mutants] [--android] [--no-interop] [--fuzz-seconds N]" >&2
      exit 2
      ;;
  esac
  shift
done

step() {
  echo
  echo "[deep-validation] $*"
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 127
  fi
}

need cargo
need python3
mkdir -p "${artifact_dir}"

step "validation docs drift"
bash scripts/validation-doc-drift.sh

step "prns-core default tests"
cargo test -p prns-core

step "prns-interfaces-tokio all-features lane"
(cd prns-interfaces/impls/tokio && cargo test --all-features)

if [ "${run_interop}" = "1" ]; then
  step "RNS 1.3.8 shared-instance msgpack RPC oracle"
  bash scripts/local-rpc-interop-smoke.sh

  step "RNS 1.3.8 remote-management oracle"
  bash scripts/remote-management-interop-smoke.sh

  step "RNS 1.3.8 probe-responder oracle"
  bash scripts/probe-responder-interop-smoke.sh

  step "RNS 1.3.8 blackhole-exchange oracle"
  bash scripts/blackhole-exchange-interop-smoke.sh

  step "RNS 1.3.5 IFAC TCP resource oracle"
  bash scripts/ifac-tcp-interop-smoke.sh
fi

step "mutation lane file list"
cargo mutants --list-files

if [ "${mode}" != "quick" ]; then
  step "cargo-fuzz build check"
  cargo +nightly fuzz check

  if [ "${fuzz_seconds}" != "0" ]; then
    while IFS= read -r target; do
      step "short fuzz run: ${target}"
      mkdir -p "fuzz/artifacts/${target}"
      cargo +nightly fuzz run "${target}" -- \
        -max_total_time="${fuzz_seconds}" \
        -artifact_prefix="fuzz/artifacts/${target}/"
    done < <(sed -n 's/^cargo +nightly fuzz run \([A-Za-z0-9_]*\) --.*/\1/p' docs/validation.md)
  fi

  while IFS= read -r harness; do
    step "Kani proof: ${harness}"
    cargo kani -p prns-core --harness "${harness}"
  done < <(sed -n 's/^cargo kani -p prns-core --harness \([A-Za-z0-9_]*\)$/\1/p' docs/validation.md)
fi

if [ "${run_mutants}" = "1" ]; then
  step "full mutation lane"
  bash scripts/mutation-triage.sh
fi

if [ "${run_android}" = "1" ]; then
  step "Android foreground-service runtime smoke"
  bash scripts/android-runtime-smoke.sh
fi

step "collect validation artifact manifest"
bash scripts/collect-validation-artifacts.sh

echo
echo "DEEP_VALIDATION_OK"
