#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATA="$ROOT/examples/observability/data"
CONFIG="${PRNSD_CONFIG:-$ROOT/examples/observability/demo}"

mkdir -p "$DATA"
cd "$ROOT"

cargo build --manifest-path prnsd/Cargo.toml --features otlp

export RUST_LOG="${RUST_LOG:-info,prns.interface=debug,prns_interfaces_tokio=debug}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4318}"
export OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-prnsd}"
export OTEL_TRACES_SAMPLER="${OTEL_TRACES_SAMPLER:-always_on}"
export OTEL_METRIC_EXPORT_INTERVAL="${OTEL_METRIC_EXPORT_INTERVAL:-5000}"

"$ROOT/prnsd/target/debug/prnsd" --log-format json --config "$CONFIG" 2>&1 |
  tee "$DATA/prnsd.jsonl"
