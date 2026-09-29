#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec cargo run --manifest-path "${repo_root}/personal-hopspot/resources/Cargo.toml" -- "$@"
