use std::ffi::{OsStr, OsString};
use std::fmt;

const ONE_SHOT_DAEMON_COMMANDS: &[&str] = &[
    "i2p",
    "interfaces",
    "nnpages",
    "status",
    "path",
    "probe",
    "id",
    "cp",
    "x",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Start,
    Restart,
    Build,
    Stop,
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
            Self::Logs => "logs",
            Self::OneShot => "one-shot daemon command",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ArgumentError {
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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) action: Action,
    pub(super) attach: bool,
    pub(super) build_args: Vec<OsString>,
    pub(super) daemon_args: Vec<OsString>,
}

impl Invocation {
    pub(super) fn has_explicit_launch_options(&self) -> bool {
        !self.build_args.is_empty() || !self.daemon_args.is_empty()
    }
}

pub(super) fn parse_invocation(args: &[OsString]) -> Result<Invocation, ArgumentError> {
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
    ONE_SHOT_DAEMON_COMMANDS
        .iter()
        .any(|command| arg == *command)
}

pub(super) fn validate_profiles(build_args: &[OsString]) -> Result<(), ArgumentError> {
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

pub(super) fn option_present(args: &[OsString], name: &str) -> bool {
    args.iter().any(|arg| {
        arg == name
            || arg.to_str().is_some_and(|arg| {
                arg.strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
            })
    })
}

pub(super) fn help_requested(args: &[OsString]) -> bool {
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

pub(super) fn print_help() {
    println!(concat!(
        "Build and run the Personal Reticulum daemon.\n\n",
        "Usage:\n",
        "    cargo prnsd [start] [BUILD OPTIONS] [-- PRNSD OPTIONS]\n",
        "    cargo prnsd restart [BUILD OPTIONS] [-- PRNSD OPTIONS]\n",
        "    cargo prnsd build [BUILD OPTIONS]\n",
        "    cargo prnsd <stop|logs>\n",
        "    cargo prnsd i2p <COMMAND>\n",
        "    cargo prnsd interfaces [COMMAND] [OPTIONS]\n",
        "    cargo prnsd nnpages seed [--source [--source-archive ZIP]] [--config DIR]\n",
        "    cargo prnsd nnpages refresh [--config DIR]\n",
        "    cargo prnsd nnpages announce [--config DIR]\n",
        "    cargo prnsd nnpages rename \"NAME\" [--config DIR]\n",
        "    cargo prnsd status [OPTIONS]\n",
        "    cargo prnsd path [OPTIONS]\n",
        "    cargo prnsd probe [OPTIONS] FULL_NAME DESTINATION_HASH\n",
        "    cargo prnsd id [OPTIONS]\n",
        "    cargo prnsd cp [OPTIONS] [FILE] [DESTINATION]\n",
        "    cargo prnsd x [OPTIONS] [DESTINATION] [COMMAND]\n\n",
        "Lifecycle:\n",
        "    start                 Start if needed, then attach to the daemon log (default)\n",
        "    restart               Gracefully stop, rebuild, start, and attach\n",
        "    stop                  Show recent logs, then stop while streaming shutdown logs\n",
        "    logs                  Attach to the running daemon log\n",
        "    --detach              Start or reconcile without attaching\n\n",
        "Build:\n",
        "    build                 Build with --release --locked and OTLP, then print the binary path\n\n",
        "One-shot commands:\n",
        "    i2p doctor            Check I2P router and SAM 3.1 readiness without starting Prnsd\n",
        "    i2p setup             Guide installation, SAM enablement, and interface configuration\n",
        "    interfaces            Safely inspect, edit, repair, and apply interface configuration\n",
        "    nnpages seed          Establish the editable NNPages layout and starter content\n",
        "    nnpages refresh       Reconcile hosted routes and settings with the running daemon\n",
        "    nnpages announce      Announce the hosted page destination immediately\n",
        "    nnpages rename        Save the node display name and announce it when available\n",
        "    status                Query the running shared RNS instance without starting Prnsd\n",
        "    path                  Inspect paths through the running shared RNS instance\n",
        "    probe                 Probe through the running shared RNS instance\n",
        "    id                    Manage identities and local cryptographic files\n",
        "    cp                    Transfer files through the running shared RNS instance\n",
        "    x                     Execute commands through the running shared RNS instance\n\n",
        "Profiles:\n",
        "    (default)             Build and run with --release\n",
        "    --debug               Build and run with Cargo's development profile\n",
        "    -r, --release         Build and run with the release profile\n",
        "    --profile <PROFILE>   Build and run with a named Cargo profile\n\n",
        "Repeated starts reattach without rebuilding or spawning another daemon. Build and daemon\n",
        "options are applied when starting a stopped service or with restart. Ctrl-C detaches without\n",
        "stopping the daemon. Runtime log verbosity is controlled separately with RUST_LOG.\n\n",
        "Examples:\n",
        "    cargo prnsd\n",
        "    cargo prnsd --detach\n",
        "    cargo prnsd build\n",
        "    cargo prnsd restart --debug\n",
        "    cargo prnsd restart --features otlp -- --config \"$HOME/.reticulum\"\n",
        "    cargo prnsd stop\n",
        "    cargo prnsd interfaces validate\n",
        "    cargo prnsd nnpages seed\n",
        "    cargo prnsd nnpages seed --source\n",
        "    cargo prnsd nnpages rename \"My Node\"\n",
        "    cargo prnsd status\n",
        "    cargo prnsd -- --help",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::cargo_run_arguments;
    use std::path::Path;

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
    fn interfaces_is_a_direct_one_shot_daemon_invocation() {
        assert_eq!(
            invocation(&["interfaces", "validate", "--config", "/node"]),
            Invocation {
                action: Action::OneShot,
                attach: false,
                build_args: Vec::new(),
                daemon_args: args(&["interfaces", "validate", "--config", "/node"]),
            }
        );
    }

    #[test]
    fn every_nnpages_form_is_a_direct_one_shot_daemon_invocation() {
        for daemon_args in [
            &["nnpages", "seed", "--config", "/node"][..],
            &["nnpages", "refresh", "--config", "/node"][..],
            &["nnpages", "announce", "--config", "/node"][..],
            &["nnpages", "rename", "My Node", "--config", "/node"][..],
        ] {
            let parsed = invocation(daemon_args);
            assert_eq!(parsed.action, Action::OneShot);
            assert!(!parsed.attach);
            assert!(parsed.build_args.is_empty());
            assert_eq!(parsed.daemon_args, args(daemon_args));
        }
    }

    #[test]
    fn explicit_separator_can_select_an_i2p_one_shot_with_build_options() {
        let parsed = invocation(&["--debug", "--", "i2p", "doctor"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert_eq!(parsed.build_args, args(&["--debug"]));
        assert_eq!(parsed.daemon_args, args(&["i2p", "doctor"]));
    }

    #[test]
    fn status_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&["status", "--json"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(parsed.daemon_args, args(&["status", "--json"]));
    }

    #[test]
    fn path_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&["path", "--table", "--json"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(parsed.daemon_args, args(&["path", "--table", "--json"]));
    }

    #[test]
    fn probe_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&[
            "probe",
            "rnstransport.probe",
            "00112233445566778899aabbccddeeff",
        ]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(
            parsed.daemon_args,
            args(&[
                "probe",
                "rnstransport.probe",
                "00112233445566778899aabbccddeeff",
            ])
        );
    }

    #[test]
    fn id_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&["id", "--version"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(parsed.daemon_args, args(&["id", "--version"]));
    }

    #[test]
    fn cp_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&["cp", "file.bin", "00112233445566778899aabbccddeeff"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(
            parsed.daemon_args,
            args(&["cp", "file.bin", "00112233445566778899aabbccddeeff",])
        );
    }

    #[test]
    fn x_is_a_direct_one_shot_daemon_invocation() {
        let parsed = invocation(&["x", "00112233445566778899aabbccddeeff", "printf hello"]);
        assert_eq!(parsed.action, Action::OneShot);
        assert!(!parsed.attach);
        assert!(parsed.build_args.is_empty());
        assert_eq!(
            parsed.daemon_args,
            args(&["x", "00112233445566778899aabbccddeeff", "printf hello",])
        );
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
    fn help_only_belongs_to_the_launcher_before_the_separator() {
        assert!(help_requested(&args(&["--help"])));
        assert!(help_requested(&args(&["restart", "-h"])));
        assert!(!help_requested(&args(&["--", "--help"])));
        assert!(!help_requested(&args(&["i2p", "--help"])));
    }
}
