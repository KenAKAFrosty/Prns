use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const TOOL_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let repo_root = repo_root();
    let manifest = repo_root.join("personal-hopspot/platform_impls/desktop/Cargo.toml");
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--bin")
        .arg("personal-hopspot-desktop")
        .args(args);

    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            if let Some(code) = status.code() {
                eprintln!("hopspot: app exited with status {code}");
            } else {
                eprintln!("hopspot: app exited unsuccessfully");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!(
                "hopspot: failed to run app manifest {}: {error}",
                manifest.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(TOOL_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("tools/hopspot lives under tools/")
        .to_path_buf()
}

fn print_help() {
    println!(
        "Run the Personal Hopspot desktop app.\n\n\
Usage:\n    cargo run -p hopspot\n    cargo run -p hopspot -- <cargo-run args for personal-hopspot-desktop>\n\n\
Examples:\n    cargo run -p hopspot\n    cargo run -p hopspot -- --release"
    );
}
