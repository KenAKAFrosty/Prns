#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATA="$ROOT/prnsd/observability/data"
COMPOSE_FILE="$ROOT/prnsd/observability/compose.yaml"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker was not found; install Docker Desktop or a compatible Docker engine with Compose" >&2
  exit 1
fi

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "Docker Compose was not found; install the Compose plugin or docker-compose" >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "the Docker engine is unavailable; start Docker Desktop, OrbStack, or Colima" >&2
  exit 1
fi

case "${1:-up}" in
  up)
    mkdir -p "$DATA"
    "${COMPOSE[@]}" -f "$COMPOSE_FILE" up -d --wait
    echo "Grafana: http://127.0.0.1:3000/d/prns-observability/prns-health"
    echo "OTLP/HTTP: http://127.0.0.1:4318"
    echo "Daemon: OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 OTEL_METRIC_EXPORT_INTERVAL=5000 cargo prnsd --detach --features otlp -- --log-format json"
    ;;
  down)
    "${COMPOSE[@]}" -f "$COMPOSE_FILE" down
    ;;
  *)
    echo "usage: cargo observability [up|down]" >&2
    exit 2
    ;;
esac
