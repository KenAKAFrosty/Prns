use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

enum Mode {
    Up,
    Down,
}

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let mode = match (arguments.next().as_deref(), arguments.next()) {
        (None | Some("up"), None) => Mode::Up,
        (Some("down"), None) => Mode::Down,
        _ => {
            eprintln!("usage: cargo observability [up|down]");
            return ExitCode::from(2);
        }
    };

    let log_dir = match env::var_os("PRNSD_LOG_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => match prnsd_control::ServicePaths::discover() {
            Ok(paths) => paths.state_dir,
            Err(error) => {
                eprintln!("could not determine the prnsd log directory: {error}");
                return ExitCode::FAILURE;
            }
        },
    };

    let compose_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("runner manifest has an observability parent")
        .join("compose.yaml");

    let compose = match compose_command() {
        Ok(compose) => compose,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    if !probe_succeeds("docker", &["info"]) {
        eprintln!("the Docker engine is unavailable; start Docker Desktop, OrbStack, or Colima");
        return ExitCode::FAILURE;
    }

    match mode {
        Mode::Up => up(compose, &compose_file, &log_dir),
        Mode::Down => down(compose, &compose_file, &log_dir),
    }
}

fn up(compose: &[&str], compose_file: &Path, log_dir: &Path) -> ExitCode {
    if let Err(error) = std::fs::create_dir_all(log_dir) {
        eprintln!(
            "could not create the prnsd log directory {}: {error}",
            log_dir.display()
        );
        return ExitCode::FAILURE;
    }
    let status = run_compose(compose, compose_file, log_dir, &["up", "-d", "--wait"]);
    if status != ExitCode::SUCCESS {
        return status;
    }
    println!("Grafana: http://127.0.0.1:3000/d/prns-observability/prns-health");
    println!("OTLP/HTTP: http://127.0.0.1:4318");
    if cfg!(windows) {
        println!(
            "Daemon: $env:OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4318'; $env:OTEL_METRIC_EXPORT_INTERVAL='5000'; cargo prnsd restart --detach --features otlp -- --log-format json"
        );
    } else {
        println!(
            "Daemon: OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 OTEL_METRIC_EXPORT_INTERVAL=5000 cargo prnsd restart --detach --features otlp -- --log-format json"
        );
    }
    ExitCode::SUCCESS
}

fn down(compose: &[&str], compose_file: &Path, log_dir: &Path) -> ExitCode {
    run_compose(compose, compose_file, log_dir, &["down"])
}

fn run_compose(compose: &[&str], compose_file: &Path, log_dir: &Path, action: &[&str]) -> ExitCode {
    let mut command = Command::new(compose[0]);
    command
        .args(&compose[1..])
        .arg("-f")
        .arg(compose_file)
        .args(action)
        .env("PRNSD_LOG_DIR", log_dir);
    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8),
        Err(error) => {
            eprintln!("could not run {}: {error}", compose.join(" "));
            ExitCode::FAILURE
        }
    }
}

fn compose_command() -> Result<&'static [&'static str], String> {
    match probe("docker", &["compose", "version"]) {
        Probe::Succeeded => return Ok(&["docker", "compose"]),
        Probe::Missing => {
            return Err(String::from(
                "docker was not found; install Docker Desktop or a compatible Docker engine with Compose",
            ));
        }
        Probe::Failed => {}
    }
    match probe("docker-compose", &["--version"]) {
        Probe::Succeeded => Ok(&["docker-compose"]),
        _ => Err(String::from(
            "Docker Compose was not found; install the Compose plugin or docker-compose",
        )),
    }
}

enum Probe {
    Succeeded,
    Failed,
    Missing,
}

fn probe(program: &str, arguments: &[&str]) -> Probe {
    match Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Probe::Succeeded,
        Ok(_) => Probe::Failed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        Err(_) => Probe::Failed,
    }
}

fn probe_succeeds(program: &str, arguments: &[&str]) -> bool {
    matches!(probe(program, arguments), Probe::Succeeded)
}
