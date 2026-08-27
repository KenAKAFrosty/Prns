use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use prnsd_control::{LaunchSpec, LogLane, ServicePaths, ServiceRecord, ServiceState, StartOutcome};

use crate::arguments::Invocation;
use crate::build::build_daemon;
use crate::CommandError;

const DAEMON_VERSION: &str = include_str!("../../../VERSION");

pub(super) fn start_or_attach(
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
    crate::configure_local_source_environment(root)?;
    let binary = build_daemon(invocation, root, manifest, false)?;
    start_built(invocation, root, paths, signature, binary)
}

pub(super) fn start_built(
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

pub(super) fn attach(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), CommandError> {
    print_banner(&record.binary);
    eprintln!("Attached to prnsd; Ctrl-C detaches without stopping the daemon\n");
    prnsd_control::follow(paths, record).map_err(CommandError::from)
}

pub(super) fn print_banner(binary: &Path) {
    if std::io::stderr().is_terminal() {
        let _ = Command::new(binary).arg("--print-banner").status();
    }
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

pub(super) fn launch_signature(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arguments::parse_invocation;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation(values: &[&str]) -> Invocation {
        parse_invocation(&args(values)).unwrap()
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
}
