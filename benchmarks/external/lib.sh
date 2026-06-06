# Shared helpers for the external-implementation drivers. Each `external/<impl>/run.sh`
# sources this, clones a pinned upstream into its gitignored `.upstream/`, builds our
# harness against it, and pipes the harness's `RESULT resolved=<n> per_sec=<f>` line here
# to be written as result rows in the same schema every other driver emits.

# The canonical host id — the rustc target triple, so external rows group under the
# same results/<host>/ dir as personal-rns and the RNS reference.
rustc_host() {
  rustc -vV | awk '/^host:/{print $2}'
}

# Parse `RESULT resolved=<int> per_sec=<float>` out of a harness's stdout.
parse_result() { printf '%s\n' "$1" | sed -n 's/.*RESULT resolved=\([0-9]*\) per_sec=\([0-9.]*\).*/\1 \2/p' | tail -1; }

# clone_pinned <repo-url> <ref> <dest-dir>: full clone + checkout the pinned ref (idempotent).
clone_pinned() {
  local repo="$1" ref="$2" dest="$3"
  if [ ! -d "$dest/.git" ]; then
    git clone "$repo" "$dest"
  fi
  git -C "$dest" checkout --quiet "$ref"
}

# emit_rows <out> <impl> <commit> <toolchain> <host> <resolved> <per_sec>: write the two
# cross-comparable rows (conformance + throughput) for announce-256 to <out>, overwriting.
emit_rows() {
  local out="$1" impl="$2" commit="$3" toolchain="$4" host="$5" resolved="$6" per_sec="$7"
  mkdir -p "$(dirname "$out")"
  {
    printf '{"scenario":"announce-256","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"conformance","metric":"routes_resolved","value":%s,"unit":"count"}\n' \
      "$impl" "$commit" "$toolchain" "$host" "$resolved"
    printf '{"scenario":"announce-256","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"throughput","metric":"ingest_announces_per_sec","value":%s,"unit":"announce/s"}\n' \
      "$impl" "$commit" "$toolchain" "$host" "$per_sec"
  } >"$out"
  echo "wrote $out  (resolved $resolved/256, $per_sec announce/s)"
}

# Parse the parallel harnesses' richer line:
#   RESULT resolved=<int> lo=<int> lo_per_sec=<float> hi=<int> hi_per_sec=<float>
parse_mt() {
  printf '%s\n' "$1" | sed -n 's/.*RESULT resolved=\([0-9]*\) lo=\([0-9]*\) lo_per_sec=\([0-9.]*\) hi=\([0-9]*\) hi_per_sec=\([0-9.]*\).*/\1 \2 \3 \4 \5/p' | tail -1
}

# emit_mt_rows <out> <impl> <commit> <toolchain> <host> <resolved> <lo> <lo_per_sec> <hi> <hi_per_sec> [conformance_metric]:
# write the announce-parallel rows — one conformance row plus a throughput row per swept
# thread count (`threads`-tagged) — to <out>, overwriting. Verify-only ports pass
# `announces_verified` as the conformance metric; full-store ports default to routes_resolved.
emit_mt_rows() {
  local out="$1" impl="$2" commit="$3" toolchain="$4" host="$5" resolved="$6" \
        lo="$7" lo_ps="$8" hi="$9" hi_ps="${10}" metric="${11:-routes_resolved}"
  mkdir -p "$(dirname "$out")"
  {
    printf '{"scenario":"announce-parallel","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"conformance","metric":"%s","value":%s,"unit":"count"}\n' \
      "$impl" "$commit" "$toolchain" "$host" "$metric" "$resolved"
    printf '{"scenario":"announce-parallel","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"throughput","metric":"ingest_announces_per_sec","value":%s,"unit":"announce/s","threads":%s}\n' \
      "$impl" "$commit" "$toolchain" "$host" "$lo_ps" "$lo"
    printf '{"scenario":"announce-parallel","scenario_version":1,"implementation":"%s","commit":"%s","toolchain":"%s","host":"%s","axis":"throughput","metric":"ingest_announces_per_sec","value":%s,"unit":"announce/s","threads":%s}\n' \
      "$impl" "$commit" "$toolchain" "$host" "$hi_ps" "$hi"
  } >"$out"
  echo "wrote $out  (resolved $resolved/2560; ${lo}t=$lo_ps, ${hi}t=$hi_ps announce/s)"
}
