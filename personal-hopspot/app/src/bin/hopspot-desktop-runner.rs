use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

const APP_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const SITE_BIND: &str = "127.0.0.1:8765";
const STATIC_SITE_OFF_ENV: &str = "HOPSPOT_STATIC_SITE_OFF";
const SKIP_DX_ENV: &str = "HOPSPOT_DESKTOP_SKIP_DX";
const DRY_RUN_ENV: &str = "HOPSPOT_DESKTOP_DRY_RUN";
const DOCS_TOOLCHAIN_ENV: &str = "HOPSPOT_DESKTOP_DOCS_TOOLCHAIN";
const DEFAULT_DOCS_TOOLCHAIN: &str = "stable";
const DX_READY_TIMEOUT: Duration = Duration::from_secs(20);
const DX_READY_POLL: Duration = Duration::from_millis(150);

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let app_dir = PathBuf::from(APP_MANIFEST_DIR);
    let repo_root = app_dir
        .parent()
        .and_then(Path::parent)
        .expect("app crate lives under personal-hopspot/app")
        .to_path_buf();
    let docs_dir = repo_root.join("docs/website");
    let serve_docs = env::var_os(SKIP_DX_ENV).is_none();

    if env::var_os(DRY_RUN_ENV).is_some() {
        print_dry_run(&args, &docs_dir, serve_docs);
        return ExitCode::SUCCESS;
    }

    let mut dx = if serve_docs {
        match ensure_dx_server(&docs_dir) {
            Ok(child) => Some(child),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--bin")
        .arg("personal-hopspot-app")
        .args(&args)
        .current_dir(&app_dir);
    if serve_docs {
        command.env(STATIC_SITE_OFF_ENV, "1");
    }

    if serve_docs {
        println!("desktop: running Hopspot with live docs from http://localhost:8765/");
    } else {
        println!("desktop: running Hopspot with its built-in docs server");
    }
    let result = match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => exit_with("desktop app exited unsuccessfully", status.code()),
        Err(error) => {
            eprintln!("desktop: failed to run cargo: {error}");
            ExitCode::FAILURE
        }
    };

    if let Some(server) = dx.as_mut() {
        server.stop();
    }
    result
}

enum DxServer {
    Existing,
    Spawned(Child),
}

impl DxServer {
    fn stop(&mut self) {
        match self {
            DxServer::Existing => {}
            DxServer::Spawned(child) => stop_child(child),
        }
    }
}

fn ensure_dx_server(docs_dir: &Path) -> Result<DxServer, String> {
    if port_is_listening() {
        if existing_server_looks_like_dx() {
            println!("desktop: reusing existing dx serve at http://localhost:8765/");
            return Ok(DxServer::Existing);
        }
        return Err(format!("desktop: {SITE_BIND} is already in use by a non-Dioxus server; stop the existing docs/Hopspot server first"));
    }

    let docs_toolchain =
        env::var(DOCS_TOOLCHAIN_ENV).unwrap_or_else(|_| DEFAULT_DOCS_TOOLCHAIN.to_owned());
    let mut child = Command::new("dx")
        .args([
            "serve",
            "--addr",
            "127.0.0.1",
            "--port",
            "8765",
            "--open",
            "false",
        ])
        .current_dir(docs_dir)
        .env("RUSTUP_TOOLCHAIN", &docs_toolchain)
        .spawn()
        .map_err(|error| {
            format!(
                "desktop: failed to run dx serve in {}: {error}",
                docs_dir.display()
            )
        })?;

    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("desktop: failed to check dx serve: {error}"))?
        {
            return Err(format!("desktop: dx serve exited early ({status})"));
        }
        if port_is_listening() {
            println!("desktop: dx serve is ready at http://localhost:8765/");
            return Ok(DxServer::Spawned(child));
        }
        if started.elapsed() > DX_READY_TIMEOUT {
            stop_child(&mut child);
            return Err(format!(
                "desktop: dx serve did not become ready at {SITE_BIND} within {:?}",
                DX_READY_TIMEOUT
            ));
        }
        thread::sleep(DX_READY_POLL);
    }
}

fn port_is_listening() -> bool {
    let addr: SocketAddr = SITE_BIND.parse().expect("SITE_BIND is a socket address");
    TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok()
}

fn existing_server_looks_like_dx() -> bool {
    let addr: SocketAddr = SITE_BIND.parse().expect("SITE_BIND is a socket address");
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /browser-node-playground HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    response.lines().any(|line| {
        line.to_ascii_lowercase()
            .starts_with("cross-origin-opener-policy:")
    })
}

fn stop_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn print_dry_run(args: &[OsString], docs_dir: &Path, serve_docs: bool) {
    if serve_docs {
        println!(
            "desktop: dry run would start dx serve --addr 127.0.0.1 --port 8765 --open false in {}",
            docs_dir.display()
        );
        println!("desktop: dry run would set {STATIC_SITE_OFF_ENV}=1 for the Hopspot app");
    } else {
        println!("desktop: dry run would skip dx serve ({SKIP_DX_ENV} set)");
    }
    println!(
        "desktop: dry run would execute cargo run --bin personal-hopspot-app {}",
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn print_help() {
    println!(
        "Run Personal Hopspot desktop with a live docs dev server.\n\n\
Usage:\n    cargo desktop [cargo-run-options]\n\n\
Examples:\n    cargo desktop\n    cargo desktop --release\n\n\
Environment:\n    {SKIP_DX_ENV}=1      Skip dx serve and let Hopspot use its built-in docs server\n    \
    {DRY_RUN_ENV}=1      Print the dx/app commands and exit\n    \
    {DOCS_TOOLCHAIN_ENV}=stable  Rust toolchain for dx serve"
    );
}

fn exit_with(message: &str, code: Option<i32>) -> ExitCode {
    match code {
        Some(code) => eprintln!("desktop: {message} (exit {code})"),
        None => eprintln!("desktop: {message}"),
    }
    ExitCode::FAILURE
}
