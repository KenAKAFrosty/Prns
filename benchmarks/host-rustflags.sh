#!/usr/bin/env bash
# Host-only benchmark build knobs. Apple Silicon supports ARMv8 AES in hardware,
# but RustCrypto's `aes` crate only compiles that backend when `aes_armv8` is set.
# Keep this local to benchmark builds so embedded/no-std targets do not inherit it.

append_benchmark_host_rustflags() {
  case "$(uname -s 2>/dev/null)-$(uname -m 2>/dev/null)" in
    Darwin-arm64|Darwin-aarch64)
      local bench_flags="-C target-cpu=native --cfg aes_armv8"
      if [ -n "${RUSTFLAGS:-}" ]; then
        export RUSTFLAGS="$RUSTFLAGS $bench_flags"
      else
        export RUSTFLAGS="$bench_flags"
      fi
      ;;
  esac
}
