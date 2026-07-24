use std::env;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const COMMAND_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn main() {
    let root = repo_root();
    let runner = root.join("tools").join("prns");
    let status = Command::new("python3")
        .arg(&runner)
        .args(env::args_os().skip(1))
        .current_dir(&root)
        .status();

    match status {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!(
                "cargo tools: failed to run `python3 {}`: {error}",
                runner.display()
            );
            eprintln!(
                "cargo tools: use `./tools/prns` for bootstrap checks before Cargo is available"
            );
            process::exit(1);
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(COMMAND_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("tools command lives under tools/repo/cargo-tools-command")
        .to_path_buf()
}
