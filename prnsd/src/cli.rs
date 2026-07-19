use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use personal_rns::i2p::{I2pPeerAddress, I2pPeerAddressError, SamBridgeAddress};

use crate::utilities::rnstatus::RnstatusArgs;

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

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct I2pArgs {
    #[command(subcommand)]
    pub command: I2pCommand,
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
pub enum I2pCommand {
    #[command(about = "Check I2P router and SAM 3.1 readiness")]
    Doctor(I2pDoctorArgs),
    #[command(about = "Guide I2P installation, SAM enablement, and Prns configuration")]
    Setup(I2pSetupArgs),
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct I2pSamArgs {
    #[arg(
        long,
        value_name = "HOST:PORT",
        default_value_t,
        help = "SAM bridge to probe"
    )]
    pub sam_bridge: SamBridgeAddress,

    #[arg(
        long,
        help = "Allow plaintext SAM over an explicitly trusted non-loopback path"
    )]
    pub allow_remote_sam: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct I2pDoctorArgs {
    #[command(flatten)]
    pub sam: I2pSamArgs,
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct I2pSetupArgs {
    #[command(flatten)]
    pub sam: I2pSamArgs,

    #[arg(
        long,
        value_name = "NAME_OR_DESTINATION",
        value_parser = parse_i2p_peer,
        help = "Add an outbound I2P peer to the emitted interface stanza"
    )]
    pub peer: Vec<I2pPeerAddress>,

    #[arg(long, help = "Make the emitted I2P interface accept inbound peers")]
    pub connectable: bool,

    #[arg(
        long = "open",
        help = "Open the applicable official download or local SAM configuration page"
    )]
    pub open_guidance: bool,
}

fn parse_i2p_peer(value: &str) -> Result<I2pPeerAddress, I2pPeerAddressError> {
    I2pPeerAddress::new(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    Start(LaunchArgs),
    Restart(LaunchArgs),
    Stop,
    Logs,
    Run(DaemonArgs),
    #[command(about = "Inspect I2P connectivity")]
    I2p(I2pArgs),
    #[command(about = "Show Reticulum interface and transport status")]
    Status(RnstatusArgs),
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
                    | "logs"
                    | "run"
                    | "i2p"
                    | "status"
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
    fn i2p_doctor_uses_the_safe_default_bridge() {
        assert_eq!(
            parse(&["prnsd", "i2p", "doctor"]),
            Command::I2p(I2pArgs {
                command: I2pCommand::Doctor(I2pDoctorArgs {
                    sam: I2pSamArgs {
                        sam_bridge: SamBridgeAddress::default(),
                        allow_remote_sam: false,
                    },
                }),
            })
        );
    }

    #[test]
    fn i2p_doctor_accepts_an_explicit_remote_bridge_acknowledgement() {
        assert_eq!(
            parse(&[
                "prnsd",
                "i2p",
                "doctor",
                "--sam-bridge",
                "router.internal:7656",
                "--allow-remote-sam",
            ]),
            Command::I2p(I2pArgs {
                command: I2pCommand::Doctor(I2pDoctorArgs {
                    sam: I2pSamArgs {
                        sam_bridge: SamBridgeAddress::new("router.internal:7656").unwrap(),
                        allow_remote_sam: true,
                    },
                }),
            })
        );
    }

    #[test]
    fn i2p_setup_parses_typed_stanza_and_browser_choices() {
        assert_eq!(
            parse(&[
                "prnsd",
                "i2p",
                "setup",
                "--peer",
                "example.i2p",
                "--connectable",
                "--open",
            ]),
            Command::I2p(I2pArgs {
                command: I2pCommand::Setup(I2pSetupArgs {
                    sam: I2pSamArgs {
                        sam_bridge: SamBridgeAddress::default(),
                        allow_remote_sam: false,
                    },
                    peer: vec![I2pPeerAddress::new("example.i2p").unwrap()],
                    connectable: true,
                    open_guidance: true,
                }),
            })
        );
    }

    #[test]
    fn status_parses_stock_remote_and_display_options() {
        let Command::Status(args) = parse(&[
            "prnsd",
            "status",
            "--config",
            "/node",
            "-R",
            "00112233445566778899aabbccddeeff",
            "-i",
            "/operator",
            "-w",
            "7.5",
            "-l",
            "-t",
            "-s",
            "traffic",
            "LAN",
        ]) else {
            panic!("status must remain a direct utility command");
        };
        assert_eq!(args.config, Some(PathBuf::from("/node")));
        assert_eq!(
            args.remote,
            Some(personal_rns::identity::IdentityHash::new([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]))
        );
        assert_eq!(args.management_identity, Some(PathBuf::from("/operator")));
        assert_eq!(
            args.remote_timeout.get(),
            std::time::Duration::from_secs_f64(7.5)
        );
        assert!(args.link_stats);
        assert!(args.totals);
        assert_eq!(
            args.sort,
            Some(crate::utilities::rnstatus::RnstatusSort::Traffic)
        );
        assert_eq!(args.filter.as_deref(), Some("LAN"));
    }

    #[test]
    fn status_remote_arguments_require_each_other() {
        for values in [
            ["prnsd", "status", "-R", "00112233445566778899aabbccddeeff"],
            ["prnsd", "status", "-i", "/operator"],
        ] {
            let error = parse_from(values.into_iter().map(OsString::from)).unwrap_err();
            assert_eq!(error.exit_code(), 2);
        }
    }

    #[test]
    fn status_owns_its_stock_version_flag() {
        let Command::Status(args) = parse(&["prnsd", "status", "--version"]) else {
            panic!("status must remain a direct utility command");
        };
        assert!(args.version);
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
