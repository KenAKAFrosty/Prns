#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec "${repository_root}/applications/ci/verify_lxmf.sh"
