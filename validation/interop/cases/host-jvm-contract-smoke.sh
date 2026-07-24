#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-jvm.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked
env \
    GRADLE_USER_HOME="$scratch/gradle" \
    LD_LIBRARY_PATH="${root}/prns-host/abi/c/target/debug" \
    prns-host/bindings/jvm/gradlew \
        --project-dir prns-host/bindings/jvm \
        test \
        --no-daemon \
        --non-interactive \
        "-Dpersonal.rns.library=${root}/prns-host/abi/c/target/debug/libprns_host.so"

echo "HOST_JVM_CONTRACT_SMOKE_OK"
