#!/usr/bin/env bash
# Build a small manifest for CI/local deep-validation artifacts. The workflow
# uploads this directory plus fuzz corpora/artifacts and mutants.out, so a
# failed hardening run keeps the repro material that matters.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "${repo_root}"

artifact_dir="${PRNS_VALIDATION_ARTIFACTS:-validation-artifacts}"
mkdir -p "${artifact_dir}"

manifest="${artifact_dir}/manifest.txt"
{
  echo "prns validation artifacts"
  echo "generated_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "git_head=$(git rev-parse --verify HEAD 2>/dev/null || echo unknown)"
  echo
  echo "[fuzz artifacts]"
  find fuzz/artifacts -type f 2>/dev/null | sort || true
  echo
  echo "[fuzz corpus]"
  find fuzz/corpus -type f 2>/dev/null | sort || true
  echo
  echo "[mutants]"
  find mutants.out mutants.out.old -maxdepth 2 -type f 2>/dev/null | sort || true
  echo
  echo "[kani]"
  find target -path '*/kani*' -type f 2>/dev/null | sort | head -n 200 || true
} > "${manifest}"

echo "VALIDATION_ARTIFACT_MANIFEST ${manifest}"
