#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../../prns-napi"

mode="${1:-build}"

npm ci --ignore-scripts

case "$mode" in
  build)
    npm run build
    ;;
  debug)
    npm run build:debug
    ;;
  test)
    npm run build:debug
    npm test
    ;;
  *)
    echo "usage: napi.sh [build|debug|test]" >&2
    exit 2
    ;;
esac
