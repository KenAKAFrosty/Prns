#!/usr/bin/env bash
# Build every contestant's sustained energy harness against the pinned upstreams (shared with
# external/<impl>/), then self-test each (2s, no power). Run WITHOUT sudo (cargo/go/crystal
# caches stay user-owned); then `sudo ./measure.sh` to file the energy rows.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="$(cd "$HERE/.." && pwd)"
source "$BENCH/external/lib.sh"
CORPUS="$BENCH/scenarios/announce-energy/packets.hex"
export CRYSTAL_WORKERS="$(sysctl -n hw.logicalcpu 2>/dev/null || getconf _NPROCESSORS_ONLN)"

clone_pinned "https://codeberg.org/Lew_Palm/leviculum.git"        6f366ca "$BENCH/external/leviculum/.upstream"
clone_pinned "https://github.com/FreeTAKTeam/LXMF-rs.git"         30da190 "$BENCH/external/lxmf-rs/.upstream"
clone_pinned "https://github.com/svanichkin/go-reticulum.git"     06621cc "$BENCH/external/go-reticulum/.upstream"
clone_pinned "https://github.com/jtippett/rns-cr.git"             514c309 "$BENCH/external/rns-cr/.upstream"
clone_pinned "https://github.com/attermann/microReticulum.git"    79b8524 "$BENCH/external/microreticulum/.upstream"
clone_pinned "https://codeberg.org/skyguy/retinet.git"            6039094 "$BENCH/external/retinet/.upstream"

echo "== Prns (ours) =="
( cd "$BENCH" && cargo build --quiet --release --bin sustained )

echo "== Leviculum =="
cargo build --quiet --release --manifest-path "$HERE/contestants/leviculum/Cargo.toml"

echo "== LXMF-rs =="
LXMF_CLONE="$BENCH/external/lxmf-rs/.upstream"
mkdir -p "$LXMF_CLONE/crates/libs/rns-core/examples"
cp "$HERE/contestants/lxmf/announce_sustained.rs" "$LXMF_CLONE/crates/libs/rns-core/examples/announce_sustained.rs"
( cd "$LXMF_CLONE" && cargo build --quiet --release --example announce_sustained -p reticulum-rs-core )

echo "== go-reticulum =="
GO_CLONE="$BENCH/external/go-reticulum/.upstream"
mkdir -p "$GO_CLONE/sustainedbench"
cp "$HERE/contestants/go/main.go" "$GO_CLONE/sustainedbench/main.go"
( cd "$GO_CLONE" && go build -o "$HERE/contestants/go/sustained-go" ./sustainedbench )

echo "== rns-cr =="
CR_CLONE="$BENCH/external/rns-cr/.upstream"
cp "$HERE/contestants/rns-cr/bench_sustained.cr" "$CR_CLONE/bench_sustained.cr"
( cd "$CR_CLONE" && shards install --without-development --quiet && \
  crystal build --release -Dpreview_mt -o "$HERE/contestants/rns-cr/bench_sustained" bench_sustained.cr )

echo "== microReticulum =="
cmake -S "$HERE/contestants/microreticulum" -B "$HERE/contestants/microreticulum/build" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$HERE/contestants/microreticulum/build" -j8 >/dev/null

echo "== RetiNet venv =="
RETINET_VENV="$BENCH/external/retinet/.upstream/.venv"
if [ ! -x "$RETINET_VENV/bin/python" ]; then
  python3 -m venv "$RETINET_VENV"
  "$RETINET_VENV/bin/pip" install -q --upgrade pip
  "$RETINET_VENV/bin/pip" install -q "$BENCH/external/retinet/.upstream"
fi

echo
echo "== self-test (2s each, no power) =="
ALLCORES=$(sysctl -n hw.logicalcpu)
test_one() { printf "  %-16s " "$1"; shift; "$@" 2>/dev/null | sed -n 's/.*\(announces_per_sec=[0-9.]*\).*/\1/p' | tail -1; }
test_one "Prns"          "$BENCH/target/release/sustained" "$CORPUS" 2 50000
test_one "Leviculum"     "$HERE/contestants/leviculum/target/release/leviculum-sustained" "$CORPUS" 2 50000
test_one "LXMF-rs"       "$LXMF_CLONE/target/release/examples/announce_sustained" "$CORPUS" 2 50000
test_one "go-reticulum"  "$HERE/contestants/go/sustained-go" "$CORPUS" 2 50000
test_one "rns-cr"        "$HERE/contestants/rns-cr/bench_sustained" "$CORPUS" 2 50000
test_one "microReticulum" "$HERE/contestants/microreticulum/build/mr_sustained" "$CORPUS" 2 50000
test_one "RetiNet"       "$RETINET_VENV/bin/python" "$BENCH/reference/sustained.py" "$CORPUS" 2
test_one "RNS 1.3.1"     "$BENCH/reference/.venv/bin/python" "$BENCH/reference/sustained.py" "$CORPUS" 2

echo
echo "Built. Now:  sudo $HERE/measure.sh 30"
