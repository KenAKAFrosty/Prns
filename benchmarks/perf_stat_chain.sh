#!/usr/bin/env bash
# Real-hardware microarchitecture counters for a trunk (pure-forwarding) node during
# the 6-hop chain firehose: IPC, branch-misprediction rate, LLC-miss rate. This is the
# real-silicon complement to the deterministic iai cache+branch model in
# benches/engine_cycle_iai.rs — iai isolates our hot loops (near-100% L1, clean
# branches); this shows the whole process under real tokio+TCP load, where the LLC
# traffic is the async/kernel floor (socket/skb/mpsc), not our forward compute.
#
# Usage: ./perf_stat_chain.sh [window_seconds]   (default 15)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WINDOW="${1:-15}"
command -v perf >/dev/null 2>&1 || { echo "perf not found"; exit 1; }

"$HERE/run_chain.sh" self self self > /tmp/perf_stat_chain.log 2>&1 &
CHAIN=$!
trap 'kill "$CHAIN" 2>/dev/null' EXIT

# Find a trunk node by EXACT comm. NOT `pgrep -af scenario_node`: a transient bash
# subshell's argv also contains "scenario_node ... chain" and matches, so perf attaches
# to an idle shell and every counter reads <not counted>. Match comm exactly, then
# confirm "chain" in the real /proc cmdline.
TRUNK=""
for _ in $(seq 1 40); do
  for pid in $(pgrep -x scenario_node); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q " chain "; then
      TRUNK="$pid"
      break
    fi
  done
  [ -n "$TRUNK" ] && break
  sleep 0.5
done
[ -n "$TRUNK" ] || { echo "no trunk node came up"; exit 1; }

sleep 3  # let the firehose ramp before counting

# Hybrid-CPU note: run_chain.sh taskset-pins the trunk to P-cores (cpu_core), so count
# cpu_core/ events explicitly — generic event names split atom/core and read <not counted>.
PMU=""
[ -d /sys/devices/cpu_core ] && PMU="cpu_core/"
ev() { if [ -n "$PMU" ]; then echo "${PMU}$1/"; else echo "$1"; fi; }
EVENTS="$(ev cycles),$(ev instructions),$(ev branches),$(ev branch-misses),$(ev cache-references),$(ev cache-misses)"

echo "perf stat: trunk PID=$TRUNK, ${WINDOW}s window"
perf stat -p "$TRUNK" -e "$EVENTS" -- sleep "$WINDOW"
wait "$CHAIN" 2>/dev/null
grep -E "CHAIN-INIT" /tmp/perf_stat_chain.log | head -1
