#!/usr/bin/env bash
# File the announce-energy rows: one idle baseline, then a powermetrics-sampled sustained run
# per contestant (all logical cores), writing conformance + throughput + CPU power + energy to
# results/<host>/announce-energy/<slug>.jsonl. Run AFTER build.sh, WITH sudo (powermetrics needs
# root):  sudo ./measure.sh [seconds]   (default 30). Then `cargo run --bin render_results`.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"

D=${1:-30}
WS=50000
SAMPLE_MS=250
N=$(( D * 1000 / SAMPLE_MS ))

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="$(cd "$HERE/.." && pwd)"
CORPUS="$BENCH/scenarios/announce-energy/packets.hex"
HOST="$(rustc -vV | awk '/^host:/{print $2}')"
OUTDIR="$BENCH/results/$HOST/announce-energy"
ALLCORES=$(sysctl -n hw.logicalcpu)
export CRYSTAL_WORKERS="$ALLCORES"
USER_OWNER="${SUDO_USER:-$(whoami)}"
mkdir -p "$OUTDIR"

REF_PY="$BENCH/reference/.venv/bin/python"
RETINET_PY="$BENCH/external/retinet/.upstream/.venv/bin/python"
GIT_HEAD="$(sudo -u "$USER_OWNER" git -C "$BENCH" rev-parse --short HEAD)"
RUST_TC="$(rustc --version | sed 's/^rustc //')"
GO_TC="$(go version | awk '{print $3}')"
CR_TC="crystal $(crystal --version | sed -n 's/^Crystal \([0-9.]*\).*/\1/p') (preview_mt)"
CLANG_TC="$(clang++ --version | head -1)"
REF_TC="$("$REF_PY" -c 'import platform;print("CPython "+platform.python_version())')"
RETINET_TC="$("$RETINET_PY" -c 'import platform;print("CPython "+platform.python_version())')"

avg_cpu_mw() {
  awk '/CPU Power:/ { for (i=1;i<=NF;i++) if ($i=="mW") { sum+=$(i-1); c++ } } END { if (c) printf "%.1f", sum/c; else print "0" }' "$1"
}

echo "[idle] baseline (${D}s, no workload)…" >&2
powermetrics --samplers cpu_power -i "$SAMPLE_MS" -n "$N" 2>/dev/null > /tmp/en_idle.txt
IDLE=$(avg_cpu_mw /tmp/en_idle.txt)
echo "[idle] $IDLE mW" >&2

# run_one <slug> <impl> <commit> <toolchain> <conf_metric> -- <cmd...>
run_one() {
  local slug="$1" impl="$2" commit="$3" tc="$4" metric="$5"; shift 5
  [ "$1" = "--" ] && shift
  echo "[$impl] active (${D}s)…" >&2
  "$@" > /tmp/en_work.out 2>/tmp/en_work.err &
  local wpid=$!
  powermetrics --samplers cpu_power -i "$SAMPLE_MS" -n "$N" 2>/dev/null > /tmp/en_active.txt
  if ! wait "$wpid"; then echo "[$impl] FAILED:" >&2; cat /tmp/en_work.err >&2; return 0; fi
  local tput active net energy w resolved
  resolved=$(sed -n 's/.*CONFORMANCE resolved=\([0-9]*\).*/\1/p' /tmp/en_work.out | tail -1)
  tput=$(sed -n 's/.*announces_per_sec=\([0-9.]*\).*/\1/p' /tmp/en_work.out | tail -1)
  active=$(avg_cpu_mw /tmp/en_active.txt)
  net=$(awk "BEGIN{printf \"%.1f\", $active-$IDLE}")
  w=$(awk "BEGIN{printf \"%.4f\", $active/1000}")
  energy=$(awk "BEGIN{printf \"%.1f\", ($net/1000)/$tput*1e6}")
  local out="$OUTDIR/$slug.jsonl"
  {
    printf '{"scenario":"announce-energy","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"conformance","metric":"%s","value":%s,"unit":"count"}\n' "$impl" "$commit" "$tc" "$HOST" "$metric" "$resolved"
    printf '{"scenario":"announce-energy","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"throughput","metric":"sustained_announces_per_sec","value":%s,"unit":"announce/s"}\n' "$impl" "$commit" "$tc" "$HOST" "$tput"
    printf '{"scenario":"announce-energy","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"power","metric":"cpu_power_watts","value":%s,"unit":"W"}\n' "$impl" "$commit" "$tc" "$HOST" "$w"
    printf '{"scenario":"announce-energy","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"energy","metric":"energy_microjoules_per_announce","value":%s,"unit":"µJ/announce"}\n' "$impl" "$commit" "$tc" "$HOST" "$energy"
  } > "$out"
  echo "[$impl] $tput/s  ${active}mW  ${energy}uJ -> ${out##*/}" >&2
}

run_one personal-rns   "Prns"            "$GIT_HEAD" "$RUST_TC"  routes_resolved    -- "$BENCH/target/release/sustained" "$CORPUS" "$D" "$WS"
run_one leviculum      "Leviculum 0.6.3" 6f366ca     "$RUST_TC"  routes_resolved    -- "$HERE/contestants/leviculum/target/release/leviculum-sustained" "$CORPUS" "$D" "$WS"
run_one lxmf-rs        "LXMF-rs 0.2.0"   30da190     "$RUST_TC"  routes_resolved    -- "$BENCH/external/lxmf-rs/.upstream/target/release/examples/announce_sustained" "$CORPUS" "$D" "$WS"
run_one go-reticulum   "go-reticulum"    06621cc     "$GO_TC"    routes_resolved    -- "$HERE/contestants/go/sustained-go" "$CORPUS" "$D" "$WS"
run_one rns-cr         "rns-cr 0.1.0"    514c309     "$CR_TC"    announces_verified -- "$HERE/contestants/rns-cr/bench_sustained" "$CORPUS" "$D" "$WS"
run_one microreticulum "microReticulum"  79b8524     "$CLANG_TC" announces_verified -- "$HERE/contestants/microreticulum/build/mr_sustained" "$CORPUS" "$D" "$WS"
run_one rns-1.3.1      "RNS 1.3.1"       "rns 1.3.1" "$REF_TC"   routes_resolved    -- "$REF_PY" "$BENCH/reference/sustained.py" "$CORPUS" "$D"
run_one retinet        "RetiNet 0.9.4"   6039094     "$RETINET_TC" routes_resolved  -- "$RETINET_PY" "$BENCH/reference/sustained.py" "$CORPUS" "$D"

chown -R "$USER_OWNER" "$OUTDIR"
echo
echo "Filed $OUTDIR (idle $IDLE mW). Now:  cargo run --release --bin render_results"
