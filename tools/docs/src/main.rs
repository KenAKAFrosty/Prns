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

    let docs_dir = repo_root().join("docs/website");
    let mut command = Command::new("dx");
    command.current_dir(&docs_dir);

    if args.is_empty() {
        command.args([
            "serve",
            "--addr",
            "127.0.0.1",
            "--port",
            "8765",
            "--open",
            "false",
        ]);
    } else {
        command.args(args);
    }

    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            if let Some(code) = status.code() {
                eprintln!("docs: dx exited with status {code}");
            } else {
                eprintln!("docs: dx exited unsuccessfully");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!(
                "docs: failed to run `dx` in {}: {error}",
                docs_dir.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(TOOL_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("tools/docs lives under tools/")
        .to_path_buf()
}

fn print_help() {
    println!(
        "Run the Prns docs site.\n\n\
Usage:\n    cargo run -p docs\n    cargo run -p docs -- <dx args>\n\n\
Examples:\n    cargo run -p docs\n    cargo run -p docs -- build --platform web --release"
    );
}
