use std::fmt;
use std::process::ExitCode;

use prnsd_control::{
    LaunchSpec, LogLane, ServiceError, ServiceKind, ServicePaths, ServiceRecord, ServiceState,
    StartOutcome, StateDirectoryError,
};

use crate::{cli, splash};

#[derive(Debug)]
enum CommandError {
    StateDirectory(StateDirectoryError),
    CurrentExecutable(std::io::Error),
    CurrentDirectory(std::io::Error),
    Service(ServiceError),
    NotRunning,
    ForegroundLogs,
    ForegroundRestart,
}

pub enum Command {
    Start(cli::LaunchArgs),
    Restart(cli::LaunchArgs),
    Stop,
    Logs,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::CurrentExecutable(error) => {
                write!(formatter, "Could not locate the prnsd executable: {error}")
            }
            Self::CurrentDirectory(error) => {
                write!(
                    formatter,
                    "Could not determine the current directory: {error}"
                )
            }
            Self::Service(error) => error.fmt(formatter),
            Self::NotRunning => formatter.write_str("prnsd is not running"),
            Self::ForegroundLogs => formatter
                .write_str("the active prnsd writes to its service manager; inspect logs there"),
            Self::ForegroundRestart => formatter
                .write_str("the active prnsd is owned by its service manager; restart it there"),
        }
    }
}

impl From<StateDirectoryError> for CommandError {
    fn from(error: StateDirectoryError) -> Self {
        Self::StateDirectory(error)
    }
}

impl From<ServiceError> for CommandError {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}

pub fn run(command: Command) -> ExitCode {
    match run_inner(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("prnsd: {error}");
            if matches!(error, CommandError::NotRunning) {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run_inner(command: Command) -> Result<(), CommandError> {
    let paths = ServicePaths::discover()?;
    match command {
        Command::Start(args) => start_or_attach(&paths, args),
        Command::Restart(args) => {
            if prnsd_control::running(&paths)?
                .is_some_and(|record| record.kind == ServiceKind::Foreground)
            {
                return Err(CommandError::ForegroundRestart);
            }
            if prnsd_control::stop(&paths)? {
                eprintln!("Stopped prnsd");
            }
            start_new(&paths, args)
        }
        Command::Stop => match prnsd_control::running(&paths)? {
            Some(record) => {
                print_managed_banner(&record);
                if record.kind == ServiceKind::Foreground {
                    eprintln!("Stopping prnsd (pid {})", record.pid);
                    prnsd_control::stop(&paths)?;
                    eprintln!("Stopped prnsd");
                    return Ok(());
                }
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
        Command::Logs => match prnsd_control::running(&paths)? {
            Some(record) if record.kind == ServiceKind::Foreground => {
                Err(CommandError::ForegroundLogs)
            }
            Some(record) => attach(&paths, &record),
            None => Err(CommandError::NotRunning),
        },
    }
}

fn start_or_attach(paths: &ServicePaths, args: cli::LaunchArgs) -> Result<(), CommandError> {
    if let Some(record) = prnsd_control::running(paths)? {
        eprintln!("prnsd is already running (pid {})", record.pid);
        let signature = daemon_signature(&args.daemon);
        if explicit_launch_configuration(&args.daemon) && record.signature != signature {
            eprintln!("Existing launch options were retained; use prnsd restart to replace them");
        }
        if args.detach {
            if record.state == ServiceState::Starting {
                prnsd_control::wait_until_ready(paths, record)?;
            }
            return Ok(());
        }
        if record.kind == ServiceKind::Foreground {
            if record.state == ServiceState::Starting {
                prnsd_control::wait_until_ready(paths, record)?;
            }
            eprintln!("The active prnsd is attached to its service manager");
            return Ok(());
        }
        return attach(paths, &record);
    }
    start_new(paths, args)
}

fn start_new(paths: &ServicePaths, args: cli::LaunchArgs) -> Result<(), CommandError> {
    let binary = std::env::current_exe().map_err(CommandError::CurrentExecutable)?;
    let working_dir = std::env::current_dir().map_err(CommandError::CurrentDirectory)?;
    let daemon_args = args.daemon.command_line();
    #[cfg(windows)]
    let managed_binary = paths.state_dir.join("prnsd-managed.exe");
    let log_lane = match args.daemon.log_format {
        cli::LogFormat::Human => LogLane::Human,
        cli::LogFormat::Json => LogLane::Json,
    };
    eprintln!("Starting prnsd...");
    let outcome = prnsd_control::start(
        paths,
        LaunchSpec {
            binary: &binary,
            #[cfg(windows)]
            managed_binary: Some(&managed_binary),
            #[cfg(not(windows))]
            managed_binary: None,
            args: &daemon_args,
            working_dir: &working_dir,
            log_lane,
            signature: daemon_signature(&args.daemon),
            version: env!("CARGO_PKG_VERSION"),
        },
    );
    let record = match outcome {
        Ok(StartOutcome::Started(record)) => {
            eprintln!(
                "Started prnsd (pid {}, log {})",
                record.pid,
                record.log(paths).display()
            );
            record
        }
        Ok(StartOutcome::AlreadyRunning(record)) => {
            eprintln!("prnsd is already running (pid {})", record.pid);
            record
        }
        Err(ServiceError::ProcessExited { log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(ServiceError::ProcessExited { log }.into());
        }
        Err(ServiceError::StartupTimedOut { pid, log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(ServiceError::StartupTimedOut { pid, log }.into());
        }
        Err(error) => return Err(error.into()),
    };
    if args.detach {
        return Ok(());
    }
    attach(paths, &record)
}

fn attach(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), CommandError> {
    if record.kind == ServiceKind::Foreground {
        return Err(CommandError::ForegroundLogs);
    }
    print_managed_banner(record);
    eprintln!("Attached to prnsd; Ctrl-C detaches without stopping the daemon\n");
    prnsd_control::follow(paths, record).map_err(CommandError::from)
}

fn print_managed_banner(record: &ServiceRecord) {
    splash::print(&format!("Personal RNS Daemon · v{}", record.version));
}

fn daemon_signature(args: &cli::DaemonArgs) -> u64 {
    prnsd_control::launch_signature(args.command_line(), std::env::vars_os())
}

fn explicit_launch_configuration(args: &cli::DaemonArgs) -> bool {
    args.has_explicit_options()
        || std::env::vars_os().any(|(name, _)| {
            name == "RUST_LOG" || name.to_str().is_some_and(|name| name.starts_with("OTEL_"))
        })
}
