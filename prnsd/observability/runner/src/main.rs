use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("runner manifest has an observability parent")
        .join("run-stack.sh");
    match Command::new(script).args(env::args_os().skip(1)).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8),
        Err(error) => {
            eprintln!("could not start the observability runner: {error}");
            ExitCode::FAILURE
        }
    }
}
