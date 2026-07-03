#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

LEDGER="scripts/comment-census.txt"
STATUS=0

count_dir() {
  local dir="$1"
  git ls-files "$dir" \
    | grep '\.rs$' \
    | grep -v '^benchmarks/external/' \
    | while read -r f; do
        grep -cE '^[[:space:]]*(//[^/!]|//$|///|//!)' "$f" || true
        echo "-$(grep -cE '^[[:space:]]*// SAFETY' "$f" || true)"
      done \
    | awk '{s+=$1} END {print s+0}'
}

if [ "${1:-}" = "--write" ]; then
  while read -r dir _; do
    echo "$dir $(count_dir "$dir")"
  done < "$LEDGER" > "$LEDGER.tmp"
  mv "$LEDGER.tmp" "$LEDGER"
  cat "$LEDGER"
  exit 0
fi

while read -r dir budget; do
  count=$(count_dir "$dir")
  if [ "$count" -ne "$budget" ]; then
    echo "COMMENT_CENSUS_DRIFT ${dir}: counted ${count}, ledger says ${budget}"
    STATUS=1
  fi
done < "$LEDGER"

if [ "$STATUS" -ne 0 ]; then
  echo "comment lines changed: raze them, or update ${LEDGER} in the same commit as the deliberate approval"
  exit 1
fi
echo "COMMENT_CENSUS_OK"
