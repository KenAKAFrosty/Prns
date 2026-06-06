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
