use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Args)]
pub struct DaemonArgs {
    #[arg(long, value_enum, default_value_t)]
    pub log_format: LogFormat,

    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}

impl DaemonArgs {
    pub fn command_line(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("run")];
        if self.log_format == LogFormat::Json {
            args.push(OsString::from("--log-format"));
            args.push(OsString::from("json"));
        }
        if let Some(config) = &self.config {
            args.push(OsString::from("--config"));
            args.push(config.as_os_str().to_owned());
        }
        args
    }

    pub fn has_explicit_options(&self) -> bool {
        self.log_format != LogFormat::Human || self.config.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Args)]
pub struct LaunchArgs {
    #[arg(long)]
    pub detach: bool,

    #[command(flatten)]
    pub daemon: DaemonArgs,
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    Start(LaunchArgs),
    Restart(LaunchArgs),
    Stop,
    Status,
    Logs,
    Run(DaemonArgs),
}

#[derive(Parser)]
#[command(name = "prnsd", version, about = "Personal Reticulum daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

pub fn parse_from(args: impl IntoIterator<Item = OsString>) -> Result<Command, clap::Error> {
    let mut args: Vec<_> = args.into_iter().collect();
    let first = args.get(1).and_then(|value| value.to_str());
    if first.is_none()
        || !matches!(
            first,
            Some(
                "start"
                    | "restart"
                    | "stop"
                    | "status"
                    | "logs"
                    | "run"
                    | "help"
                    | "--help"
                    | "-h"
                    | "--version"
                    | "-V"
            )
        )
    {
        args.insert(1, OsString::from("start"));
    }
    Cli::try_parse_from(args)
        .map(|cli| cli.command.unwrap_or(Command::Start(LaunchArgs::default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Command {
        parse_from(values.iter().map(OsString::from)).unwrap()
    }

    #[test]
    fn no_arguments_start_and_attach() {
        assert_eq!(parse(&["prnsd"]), Command::Start(LaunchArgs::default()));
    }

    #[test]
    fn start_options_work_without_the_explicit_subcommand() {
        assert_eq!(
            parse(&[
                "prnsd",
                "--detach",
                "--config",
                "/node",
                "--log-format",
                "json",
            ]),
            Command::Start(LaunchArgs {
                detach: true,
                daemon: DaemonArgs {
                    log_format: LogFormat::Json,
                    config: Some(PathBuf::from("/node")),
                },
            })
        );
    }

    #[test]
    fn foreground_run_is_explicit() {
        assert_eq!(
            parse(&["prnsd", "run", "--config", "/node"]),
            Command::Run(DaemonArgs {
                log_format: LogFormat::Human,
                config: Some(PathBuf::from("/node")),
            })
        );
    }

    #[test]
    fn daemon_command_line_is_stable() {
        let args = DaemonArgs {
            log_format: LogFormat::Json,
            config: Some(PathBuf::from("/node")),
        };
        assert_eq!(
            args.command_line(),
            vec!["run", "--log-format", "json", "--config", "/node"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn help_and_version_are_successful_one_shots() {
        for flag in ["--help", "-h", "--version", "-V"] {
            let error = parse_from([OsString::from("prnsd"), OsString::from(flag)]).unwrap_err();
            assert_eq!(error.exit_code(), 0);
        }
    }
}
