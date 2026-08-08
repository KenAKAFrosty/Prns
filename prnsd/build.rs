use std::env;

fn main() {
    // Windows gives the main thread 1 MiB of stack where Linux and macOS give 8 MiB,
    // and the unoptimized poll frame of run_command needs most of a MiB by itself,
    // so debug binaries overflow at startup for every real subcommand. Reserve the
    // same headroom the other platforms already assume.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}
