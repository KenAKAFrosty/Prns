use std::env;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};

const COMMAND_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

const INTERPRETER_CANDIDATES: &[&str] = if cfg!(windows) {
    &["python", "py", "python3"]
} else {
    &["python3", "python"]
};

fn main() {
    let root = repo_root();
    let runner = root.join("tools").join("prns");
    let Some(interpreter) = working_interpreter() else {
        eprintln!(
            "cargo tools: no working Python interpreter found (tried {})",
            INTERPRETER_CANDIDATES.join(", ")
        );
        eprintln!("cargo tools: install Python 3.11 or newer, then retry");
        process::exit(1);
    };
    let status = Command::new(interpreter)
        .arg(&runner)
        .args(env::args_os().skip(1))
        .current_dir(&root)
        .status();

    match status {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!(
                "cargo tools: failed to run `{interpreter} {}`: {error}",
                runner.display()
            );
            process::exit(1);
        }
    }
}

fn working_interpreter() -> Option<&'static str> {
    INTERPRETER_CANDIDATES.iter().copied().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn repo_root() -> PathBuf {
    PathBuf::from(COMMAND_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("tools command lives under tools/repo/cargo-tools-command")
        .to_path_buf()
}
