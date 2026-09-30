#!/usr/bin/env bash
set -euo pipefail

applications_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repository_root="$(git -C "${applications_root}" rev-parse --show-toplevel)"
local_git_url="$(python3 -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).resolve().as_uri())' "${repository_root}")"

exec python3 "${applications_root}/tools/detached-consumer-check/run.py" \
  --prns-git-url "${PRNS_MOBILITY_GIT_URL:-${local_git_url}}" \
  "$@"
