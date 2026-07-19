use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use prnsd_control::{LaunchSpec, LogLane, ServicePaths, ServiceRecord, ServiceState, StartOutcome};

const TOOL_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const DAEMON_VERSION: &str = include_str!("../../../VERSION");
const I2P_DAEMON_COMMAND: &str = "i2p";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Start,
    Restart,
    Build,
    Stop,
    Status,
    Logs,
    OneShot,
}

impl Action {
    fn parse(value: &OsStr) -> Option<Self> {
        match value.to_str()? {
            "start" => Some(Self::Start),
            "restart" => Some(Self::Restart),
            "build" => Some(Self::Build),
            "stop" => Some(Self::Stop),
            "status" => Some(Self::Status),
            "logs" => Some(Self::Logs),
            _ => None,
        }
    }

    fn accepts_build_options(self) -> bool {
        matches!(self, Self::Start | Self::Restart | Self::Build)
    }
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Start => "start",
            Self::Restart => "restart",
            Self::Build => "build",
            Self::Stop => "stop",
            Self::Status => "status",
            Self::Logs => "logs",
            Self::OneShot => "one-shot daemon command",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ArgumentError {
    ConflictingProfiles,
    LifecycleOptions(Action),
    OneShotLifecycle(Action),
}

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingProfiles => {
                formatter.write_str("--debug cannot be combined with --release, -r, or --profile")
            }
            Self::LifecycleOptions(action) => write!(
                formatter,
                "cargo prnsd {action} does not accept build or daemon options"
            ),
            Self::OneShotLifecycle(action) => write!(
                formatter,
                "one-shot daemon commands cannot be combined with {action}"
            ),
        }
    }
}

#[derive(Debug)]
enum CommandError {
    Arguments(ArgumentError),
    CargoSpawn(std::io::Error),
    CargoFailed(Option<i32>),
    DaemonExited(Option<i32>),
    BinaryMissing(PathBuf),
    Service(prnsd_control::ServiceError),
    StateDirectory(prnsd_control::StateDirectoryError),
    NotRunning,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(error) => error.fmt(formatter),
            Self::CargoSpawn(error) => write!(formatter, "failed to run cargo: {error}"),
            Self::CargoFailed(Some(code)) => write!(formatter, "cargo exited with status {code}"),
            Self::CargoFailed(None) => formatter.write_str("cargo exited unsuccessfully"),
            Self::DaemonExited(Some(code)) => write!(formatter, "prnsd exited with status {code}"),
            Self::DaemonExited(None) => formatter.write_str("prnsd exited unsuccessfully"),
            Self::BinaryMissing(path) => write!(
                formatter,
                "cargo completed without producing the expected daemon at {}",
                path.display()
            ),
            Self::Service(error) => error.fmt(formatter),
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::NotRunning => formatter.write_str("prnsd is not running"),
        }
    }
}

impl From<ArgumentError> for CommandError {
    fn from(error: ArgumentError) -> Self {
        Self::Arguments(error)
    }
}

impl From<prnsd_control::ServiceError> for CommandError {
    fn from(error: prnsd_control::ServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<prnsd_control::StateDirectoryError> for CommandError {
    fn from(error: prnsd_control::StateDirectoryError) -> Self {
        Self::StateDirectory(error)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    action: Action,
    attach: bool,
    build_args: Vec<OsString>,
    daemon_args: Vec<OsString>,
}

impl Invocation {
    fn has_explicit_launch_options(&self) -> bool {
        !self.build_args.is_empty() || !self.daemon_args.is_empty()
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if help_requested(&args) {
        print_help();
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::DaemonExited(code)) => {
            ExitCode::from(code.unwrap_or(1).clamp(1, 255) as u8)
        }
        Err(error) => {
            eprintln!("prnsd: {error}");
            match error {
                CommandError::Arguments(_) => ExitCode::from(2),
                CommandError::NotRunning => ExitCode::from(3),
                CommandError::CargoFailed(Some(code)) => ExitCode::from(code.clamp(1, 255) as u8),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(args: &[OsString]) -> Result<(), CommandError> {
    let invocation = parse_invocation(args)?;
    let root = repo_root();
    let manifest = root.join("prnsd/Cargo.toml");
    if invocation.action == Action::OneShot {
        return run_daemon_through_cargo(cargo_run_arguments(&invocation, &manifest)?, &root);
    }

    let paths = ServicePaths::discover()?;
    let signature = launch_signature(&invocation, env::vars_os());
    match invocation.action {
        Action::Start => start_or_attach(&invocation, &root, &manifest, &paths, signature),
        Action::Restart => {
            let binary = build_daemon(&invocation, &root, &manifest, false)?;
            if prnsd_control::stop(&paths)? {
                eprintln!("Stopped prnsd");
            }
            start_built(&invocation, &root, &paths, signature, binary)
        }
        Action::Build => {
            let binary = build_daemon(&invocation, &root, &manifest, true)?;
            println!("{}", binary.display());
            Ok(())
        }
        Action::Stop => match prnsd_control::running(&paths)? {
            Some(record) => {
                print_banner(&record.binary);
                eprintln!(
                    "Stopping prnsd (pid {}); showing recent and shutdown logs\n",
                    record.pid
                );
                prnsd_control::stop_and_follow(&paths, &record)?;
                eprintln!("\nStopped prnsd");
                Ok(())
            }
            None => {
                eprintln!("prnsd is already stopped");
                Ok(())
            }
        },
        Action::Status => match prnsd_control::running(&paths)? {
            Some(record) => {
                let state = match record.state {
                    ServiceState::Starting => "starting",
                    ServiceState::Running => "running",
                };
                eprintln!(
                    "prnsd is {state} (pid {}, version {}, log {})",
                    record.pid,
                    record.version,
                    record.log(&paths).display()
                );
                Ok(())
            }
            None => Err(CommandError::NotRunning),
        },
        Action::Logs => match prnsd_control::running(&paths)? {
            Some(record) => attach(&paths, &record),
            None => Err(CommandError::NotRunning),
        },
        Action::OneShot => {
            run_daemon_through_cargo(cargo_run_arguments(&invocation, &manifest)?, &root)
        }
    }
}

fn start_or_attach(
    invocation: &Invocation,
    root: &Path,
    manifest: &Path,
    paths: &ServicePaths,
    signature: u64,
) -> Result<(), CommandError> {
    if let Some(record) = prnsd_control::running(paths)? {
        eprintln!("prnsd is already running (pid {})", record.pid);
        if invocation.has_explicit_launch_options() && record.signature != signature {
            eprintln!(
                "Existing launch options were retained; use cargo prnsd restart to replace them"
            );
        }
        return attach_if_requested(invocation, paths, &record);
    }
    start_new(invocation, root, manifest, paths, signature)
}

fn start_new(
    invocation: &Invocation,
    root: &Path,
    manifest: &Path,
    paths: &ServicePaths,
    signature: u64,
) -> Result<(), CommandError> {
    let binary = build_daemon(invocation, root, manifest, false)?;
    start_built(invocation, root, paths, signature, binary)
}

fn start_built(
    invocation: &Invocation,
    root: &Path,
    paths: &ServicePaths,
    signature: u64,
    binary: PathBuf,
) -> Result<(), CommandError> {
    let log_lane = if json_logging(&invocation.daemon_args) {
        LogLane::Json
    } else {
        LogLane::Human
    };
    let daemon_args = managed_daemon_arguments(&invocation.daemon_args);
    #[cfg(windows)]
    let managed_binary = paths.state_dir.join("prnsd-managed.exe");
    let outcome = match prnsd_control::start(
        paths,
        LaunchSpec {
            binary: &binary,
            #[cfg(windows)]
            managed_binary: Some(&managed_binary),
            #[cfg(not(windows))]
            managed_binary: None,
            args: &daemon_args,
            working_dir: root,
            log_lane,
            signature,
            version: DAEMON_VERSION.trim(),
        },
    ) {
        Ok(outcome) => outcome,
        Err(prnsd_control::ServiceError::ProcessExited { log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(CommandError::Service(
                prnsd_control::ServiceError::ProcessExited { log },
            ));
        }
        Err(prnsd_control::ServiceError::StartupTimedOut { pid, log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(CommandError::Service(
                prnsd_control::ServiceError::StartupTimedOut { pid, log },
            ));
        }
        Err(error) => return Err(CommandError::Service(error)),
    };
    let record = match outcome {
        StartOutcome::Started(record) => {
            eprintln!(
                "Started prnsd (pid {}, log {})",
                record.pid,
                record.log(paths).display()
            );
            record
        }
        StartOutcome::AlreadyRunning(record) => {
            eprintln!("prnsd is already running (pid {})", record.pid);
            record
        }
    };
    attach_if_requested(invocation, paths, &record)
}

fn attach_if_requested(
    invocation: &Invocation,
    paths: &ServicePaths,
    record: &ServiceRecord,
) -> Result<(), CommandError> {
    if !invocation.attach {
        if record.state == ServiceState::Starting {
            prnsd_control::wait_until_ready(paths, record.clone())?;
        }
        return Ok(());
    }
    attach(paths, record)
}

fn attach(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), CommandError> {
    print_banner(&record.binary);
    eprintln!("Attached to prnsd; Ctrl-C detaches without stopping the daemon\n");
    prnsd_control::follow(paths, record).map_err(CommandError::from)
}

fn print_banner(binary: &Path) {
    if std::io::stderr().is_terminal() {
        let _ = Command::new(binary).arg("--print-banner").status();
    }
}

fn build_daemon(
    invocation: &Invocation,
    root: &Path,
    manifest: &Path,
    canonical: bool,
) -> Result<PathBuf, CommandError> {
    let build_args = if canonical {
        canonical_build_arguments(invocation, manifest)?
    } else {
        cargo_build_arguments(invocation, manifest)?
    };
    run_cargo(build_args, root)?;
    let binary = daemon_binary_path(
        &invocation.build_args,
        root,
        manifest,
        env::var_os("CARGO_TARGET_DIR").as_deref(),
    );
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(CommandError::BinaryMissing(binary))
    }
}

fn run_cargo(args: Vec<OsString>, working_dir: &Path) -> Result<(), CommandError> {
    let status = cargo_status(args, working_dir)?;
    if status.success() {
        Ok(())
    } else {
        Err(CommandError::CargoFailed(status.code()))
    }
}

fn run_daemon_through_cargo(args: Vec<OsString>, working_dir: &Path) -> Result<(), CommandError> {
    let status = cargo_status(args, working_dir)?;
    if status.success() {
        Ok(())
    } else {
        Err(CommandError::DaemonExited(status.code()))
    }
}

fn cargo_status(
    args: Vec<OsString>,
    working_dir: &Path,
) -> Result<std::process::ExitStatus, CommandError> {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(working_dir)
        .status()
        .map_err(CommandError::CargoSpawn)?;
    Ok(status)
}

fn parse_invocation(args: &[OsString]) -> Result<Invocation, ArgumentError> {
    if args
        .first()
        .is_some_and(|arg| is_direct_daemon_command(arg))
    {
        return Ok(Invocation {
            action: Action::OneShot,
            attach: false,
            build_args: Vec::new(),
            daemon_args: args.to_vec(),
        });
    }
    let separator = separator_index(args);
    let mut build_args = args[..separator].to_vec();
    let daemon_args = if separator < args.len() {
        args[separator + 1..].to_vec()
    } else {
        Vec::new()
    };
    let action = build_args
        .first()
        .and_then(|arg| Action::parse(arg))
        .unwrap_or(Action::Start);
    if build_args
        .first()
        .and_then(|arg| Action::parse(arg))
        .is_some()
    {
        build_args.remove(0);
    }
    let detached = build_args.iter().any(|arg| arg == "--detach");
    let attach = !detached;
    build_args.retain(|arg| arg != "--detach");
    validate_profiles(&build_args)?;

    if action == Action::Build && (detached || !daemon_args.is_empty()) {
        return Err(ArgumentError::LifecycleOptions(action));
    }
    if !action.accepts_build_options()
        && (detached || !build_args.is_empty() || !daemon_args.is_empty())
    {
        return Err(ArgumentError::LifecycleOptions(action));
    }
    let one_shot = daemon_args
        .first()
        .is_some_and(|arg| is_direct_daemon_command(arg))
        || daemon_args
            .iter()
            .any(|arg| arg == "--help" || arg == "-h" || arg == "--version" || arg == "-V");
    if one_shot && action != Action::Start {
        return Err(ArgumentError::OneShotLifecycle(action));
    }
    Ok(Invocation {
        action: if one_shot { Action::OneShot } else { action },
        attach: if one_shot { false } else { attach },
        build_args,
        daemon_args,
    })
}

fn is_direct_daemon_command(arg: &OsStr) -> bool {
    arg == I2P_DAEMON_COMMAND
}

fn validate_profiles(build_args: &[OsString]) -> Result<(), ArgumentError> {
    let debug = build_args.iter().any(|arg| arg == "--debug");
    let release = build_args
        .iter()
        .any(|arg| arg == "--release" || arg == "-r");
    let profile = option_present(build_args, "--profile");
    if debug && (release || profile) {
        Err(ArgumentError::ConflictingProfiles)
    } else {
        Ok(())
    }
}

fn cargo_build_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_build_arguments_with_mode(invocation, manifest, false)
}

fn canonical_build_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_build_arguments_with_mode(invocation, manifest, true)
}

fn cargo_build_arguments_with_mode(
    invocation: &Invocation,
    manifest: &Path,
    canonical: bool,
) -> Result<Vec<OsString>, ArgumentError> {
    let mut args = cargo_arguments("build", invocation, manifest, false)?;
    if canonical {
        if !args.iter().any(|arg| arg == "--locked") {
            args.push(OsString::from("--locked"));
        }
        args.push(OsString::from("--features"));
        args.push(OsString::from("otlp"));
    }
    Ok(args)
}

fn cargo_run_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_arguments("run", invocation, manifest, true)
}

fn cargo_arguments(
    command: &str,
    invocation: &Invocation,
    manifest: &Path,
    include_daemon_args: bool,
) -> Result<Vec<OsString>, ArgumentError> {
    validate_profiles(&invocation.build_args)?;
    let debug = invocation.build_args.iter().any(|arg| arg == "--debug");
    let release = invocation
        .build_args
        .iter()
        .any(|arg| arg == "--release" || arg == "-r");
    let profile = option_present(&invocation.build_args, "--profile");

    let mut cargo_args = vec![
        OsString::from(command),
        OsString::from("--manifest-path"),
        manifest.as_os_str().to_owned(),
    ];
    if command == "build" {
        cargo_args.push(OsString::from("--bin"));
        cargo_args.push(OsString::from("prnsd"));
    }
    if !debug && !release && !profile {
        cargo_args.push(OsString::from("--release"));
    }
    cargo_args.extend(
        invocation
            .build_args
            .iter()
            .filter(|arg| *arg != "--debug")
            .cloned(),
    );
    if include_daemon_args {
        cargo_args.push(OsString::from("--"));
        cargo_args.extend(invocation.daemon_args.iter().cloned());
    }
    Ok(cargo_args)
}

fn daemon_binary_path(
    build_args: &[OsString],
    repo_root: &Path,
    manifest: &Path,
    cargo_target_dir: Option<&OsStr>,
) -> PathBuf {
    let target_dir = option_value(build_args, "--target-dir")
        .or_else(|| cargo_target_dir.map(OsStr::to_owned))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            manifest
                .parent()
                .expect("prnsd manifest has a parent")
                .join("target")
        });
    let mut path = if target_dir.is_absolute() {
        target_dir
    } else {
        repo_root.join(target_dir)
    };
    if let Some(target) = option_value(build_args, "--target") {
        path.push(target);
    }
    path.push(profile_directory(build_args));
    path.push(if cfg!(windows) { "prnsd.exe" } else { "prnsd" });
    path
}

fn profile_directory(build_args: &[OsString]) -> OsString {
    if build_args.iter().any(|arg| arg == "--debug") {
        return OsString::from("debug");
    }
    match option_value(build_args, "--profile") {
        Some(profile) if profile == "dev" => OsString::from("debug"),
        Some(profile) => profile,
        None => OsString::from("release"),
    }
}

fn option_value(args: &[OsString], name: &str) -> Option<OsString> {
    for (index, arg) in args.iter().enumerate() {
        if arg == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix(name))
            .and_then(|arg| arg.strip_prefix('='))
        {
            return Some(OsString::from(value));
        }
    }
    None
}

fn option_present(args: &[OsString], name: &str) -> bool {
    args.iter().any(|arg| {
        arg == name
            || arg.to_str().is_some_and(|arg| {
                arg.strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
            })
    })
}

fn json_logging(daemon_args: &[OsString]) -> bool {
    daemon_args.iter().enumerate().any(|(index, arg)| {
        arg.to_str().is_some_and(|arg| arg == "--log-format=json")
            || (arg == "--log-format"
                && daemon_args
                    .get(index + 1)
                    .is_some_and(|value| value == "json"))
    })
}

fn launch_signature(
    invocation: &Invocation,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> u64 {
    let values = invocation
        .build_args
        .iter()
        .cloned()
        .chain([OsString::from("--")])
        .chain(invocation.daemon_args.iter().cloned());
    prnsd_control::launch_signature(values, environment)
}

fn managed_daemon_arguments(daemon_args: &[OsString]) -> Vec<OsString> {
    std::iter::once(OsString::from("run"))
        .chain(daemon_args.iter().cloned())
        .collect()
}

fn help_requested(args: &[OsString]) -> bool {
    !args
        .first()
        .is_some_and(|arg| is_direct_daemon_command(arg))
        && args[..separator_index(args)]
            .iter()
            .any(|arg| arg == "--help" || arg == "-h")
}

fn separator_index(args: &[OsString]) -> usize {
    args.iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())
}

fn repo_root() -> PathBuf {
    PathBuf::from(TOOL_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("tools/prnsd lives under tools/")
        .to_path_buf()
}

fn print_help() {
    println!(
        "Build and run the Personal Reticulum daemon.\n\n\
Usage:\n    cargo prnsd [start] [BUILD OPTIONS] [-- PRNSD OPTIONS]\n    cargo prnsd restart [BUILD OPTIONS] [-- PRNSD OPTIONS]\n    cargo prnsd build [BUILD OPTIONS]\n    cargo prnsd <stop|status|logs>\n    cargo prnsd i2p <COMMAND>\n\n\
Lifecycle:\n    start                 Start if needed, then attach to the daemon log (default)\n    restart               Gracefully stop, rebuild, start, and attach\n    stop                  Show recent logs, then stop while streaming shutdown logs\n    status                Show whether the managed daemon is running\n    logs                  Attach to the running daemon log\n    --detach              Start or reconcile without attaching\n\n\
Build:\n    build                 Build with --release --locked and OTLP, then print the binary path\n\n\
One-shot commands:\n    i2p doctor            Check I2P router and SAM 3.1 readiness without starting Prnsd\n    i2p setup             Guide installation, SAM enablement, and interface configuration\n\n\
Profiles:\n    (default)             Build and run with --release\n    --debug               Build and run with Cargo's development profile\n    -r, --release         Build and run with the release profile\n    --profile <PROFILE>   Build and run with a named Cargo profile\n\n\
Repeated starts reattach without rebuilding or spawning another daemon. Build and daemon\n\
options are applied when starting a stopped service or with restart. Ctrl-C detaches without\n\
stopping the daemon. Runtime log verbosity is controlled separately with RUST_LOG.\n\n\
Examples:\n    cargo prnsd\n    cargo prnsd --detach\n    cargo prnsd build\n    cargo prnsd restart --debug\n    cargo prnsd restart --features otlp -- --config \"$HOME/.reticulum\"\n    cargo prnsd stop\n    cargo prnsd -- --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation(values: &[&str]) -> Invocation {
        parse_invocation(&args(values)).unwrap()
    }

    #[test]
    fn start_and_attachment_are_the_defaults() {
        assert_eq!(
            parse_invocation(&[]).unwrap(),
            Invocation {
                action: Action::Start,
                attach: true,
                build_args: Vec::new(),
                daemon_args: Vec::new(),
            }
        );
    }

    #[test]
    fn lifecycle_and_detach_options_are_parsed_before_the_separator() {
        assert_eq!(
            parse_invocation(&args(&[
                "restart",
                "--detach",
                "--features",
                "otlp",
                "--",
                "--config",
                "path",
            ]))
            .unwrap(),
            Invocation {
                action: Action::Restart,
                attach: false,
                build_args: args(&["--features", "otlp"]),
                daemon_args: args(&["--config", "path"]),
            }
        );
    }

    #[test]
    fn build_is_build_only_and_does_not_attach() {
        assert_eq!(
            parse_invocation(&args(&["build", "--offline"])).unwrap(),
            Invocation {
                action: Action::Build,
                attach: true,
                build_args: args(&["--offline"]),
                daemon_args: Vec::new(),
            }
        );
        assert!(matches!(
            parse_invocation(&args(&["build", "--", "--config", "path"])),
            Err(ArgumentError::LifecycleOptions(Action::Build))
        ));
    }

    #[test]
    fn inspection_actions_reject_launch_options() {
        for values in [
            args(&["status", "--debug"]),
            args(&["stop", "--", "--config", "path"]),
            args(&["logs", "--detach"]),
        ] {
            assert!(matches!(
                parse_invocation(&values),
                Err(ArgumentError::LifecycleOptions(_))
            ));
        }
    }

    #[test]
    fn daemon_help_and_version_remain_one_shot() {
        for flag in ["--help", "-h", "--version", "-V"] {
            let parsed = invocation(&["--", flag]);
            assert_eq!(parsed.action, Action::OneShot);
            assert!(!parsed.attach);
            assert_eq!(parsed.daemon_args, args(&[flag]));
        }
        assert_eq!(
            parse_invocation(&args(&["restart", "--", "--version"])),
            Err(ArgumentError::OneShotLifecycle(Action::Restart))
        );
    }

    #[test]
    fn i2p_commands_are_direct_one_shot_daemon_invocations() {
        let parsed = invocation(&["i2p", "doctor", "--sam-bridge", "127.0.0.1:7656"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(
            parsed.daemon_args,
            args(&["i2p", "doctor", "--sam-bridge", "127.0.0.1:7656",])
        );
        assert_eq!(
            cargo_run_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--release",
                "--",
                "i2p",
                "doctor",
                "--sam-bridge",
                "127.0.0.1:7656",
            ]))
        );
    }

    #[test]
    fn explicit_separator_can_select_an_i2p_one_shot_with_build_options() {
        let parsed = invocation(&["--debug", "--", "i2p", "doctor"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert_eq!(parsed.build_args, args(&["--debug"]));
        assert_eq!(parsed.daemon_args, args(&["i2p", "doctor"]));
    }

    #[test]
    fn release_is_the_default_profile_for_builds() {
        assert_eq!(
            cargo_build_arguments(&invocation(&[]), Path::new("/repo/prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "/repo/prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
            ]))
        );
    }

    #[test]
    fn canonical_build_is_locked_release_with_otlp() {
        assert_eq!(
            canonical_build_arguments(&invocation(&["build"]), Path::new("/repo/prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "/repo/prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--locked",
                "--features",
                "otlp",
            ]))
        );
    }

    #[test]
    fn debug_selects_the_development_profile() {
        assert_eq!(
            cargo_build_arguments(&invocation(&["--debug"]), Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
            ]))
        );
    }

    #[test]
    fn explicit_release_and_named_profiles_are_forwarded_once() {
        for values in [
            vec!["--release"],
            vec!["-r"],
            vec!["--profile", "profiling"],
            vec!["--profile=profiling"],
        ] {
            let parsed = invocation(&values);
            let built = cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")).unwrap();
            assert_eq!(built[5..], args(&values));
        }
    }

    #[test]
    fn debug_rejects_other_profile_selectors() {
        for conflict in [
            args(&["--debug", "--release"]),
            args(&["--debug", "-r"]),
            args(&["--debug", "--profile", "dev"]),
            args(&["--debug", "--profile=dev"]),
            args(&["--debug", "--profile"]),
        ] {
            assert_eq!(
                parse_invocation(&conflict),
                Err(ArgumentError::ConflictingProfiles)
            );
        }
    }

    #[test]
    fn cargo_build_options_are_forwarded() {
        let parsed = invocation(&["--features", "otlp", "--locked", "--offline"]);
        assert_eq!(
            cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--features",
                "otlp",
                "--locked",
                "--offline",
            ]))
        );
    }

    #[test]
    fn daemon_arguments_are_excluded_from_build_and_preserved_for_one_shot_runs() {
        let parsed = invocation(&["--features", "otlp", "--", "--version"]);
        assert_eq!(
            cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--features",
                "otlp",
            ]))
        );
        assert_eq!(
            cargo_run_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--release",
                "--features",
                "otlp",
                "--",
                "--version",
            ]))
        );
    }

    #[test]
    fn binary_path_tracks_profile_target_and_target_directory() {
        let manifest = Path::new("/repo/prnsd/Cargo.toml");
        assert_eq!(
            daemon_binary_path(&[], Path::new("/repo"), manifest, None),
            Path::new("/repo/prnsd/target/release/prnsd")
        );
        assert_eq!(
            daemon_binary_path(
                &args(&[
                    "--profile=profiling",
                    "--target",
                    "aarch64-apple-darwin",
                    "--target-dir",
                    "build-output",
                ]),
                Path::new("/repo"),
                manifest,
                None,
            ),
            Path::new("/repo/build-output/aarch64-apple-darwin/profiling/prnsd")
        );
        assert_eq!(
            daemon_binary_path(
                &args(&["--profile", "dev"]),
                Path::new("/repo"),
                manifest,
                None,
            ),
            Path::new("/repo/prnsd/target/debug/prnsd")
        );
    }

    #[test]
    fn json_log_format_selects_the_grafana_log_lane() {
        assert!(json_logging(&args(&["--log-format", "json"])));
        assert!(json_logging(&args(&["--log-format=json"])));
        assert!(!json_logging(&args(&["--log-format", "human"])));
    }

    #[test]
    fn launch_signature_tracks_options_and_relevant_environment() {
        let parsed = invocation(&["--features", "otlp", "--", "--config", "path"]);
        let environment = vec![
            (OsString::from("RUST_LOG"), OsString::from("info")),
            (OsString::from("UNRELATED"), OsString::from("first")),
        ];
        let signature = launch_signature(&parsed, environment);
        assert_eq!(
            signature,
            launch_signature(
                &parsed,
                vec![
                    (OsString::from("UNRELATED"), OsString::from("second")),
                    (OsString::from("RUST_LOG"), OsString::from("info")),
                ]
            )
        );
        assert_ne!(
            signature,
            launch_signature(
                &parsed,
                vec![(OsString::from("RUST_LOG"), OsString::from("debug"))]
            )
        );
        assert_ne!(
            signature,
            launch_signature(
                &invocation(&["--features", "otlp", "--", "--config", "other"]),
                vec![(OsString::from("RUST_LOG"), OsString::from("info"))]
            )
        );
    }

    #[test]
    fn help_only_belongs_to_the_launcher_before_the_separator() {
        assert!(help_requested(&args(&["--help"])));
        assert!(help_requested(&args(&["restart", "-h"])));
        assert!(!help_requested(&args(&["--", "--help"])));
        assert!(!help_requested(&args(&["i2p", "--help"])));
    }
}
