use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "hopspot-flash",
    version,
    about = "Guided, verified firmware flasher for Personal Hopspot boards.",
    long_about = "Run without a subcommand for a guided flow. Published firmware is signature- and hash-verified before a device is opened."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<CommandMode>,
}

#[derive(Subcommand)]
pub(crate) enum CommandMode {
    /// List all publicly supported boards.
    List {
        /// Emit one stable JSON document instead of terminal formatting.
        #[arg(long)]
        json: bool,
    },
    /// List devices, or run a non-writing identity/mount preflight for BOARD.
    Doctor {
        /// Optional board slug; when present, a real non-writing preflight is required.
        board: Option<String>,
        /// Explicit serial port for an ESP board preflight.
        #[arg(long, value_name = "PORT")]
        port: Option<String>,
        /// Emit stable JSON diagnostics.
        #[arg(long)]
        json: bool,
    },
    /// Manage signature- and hash-verified release cache entries.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Download, verify, and flash published Hopspot firmware.
    Flash {
        /// Stable board slug.
        board: String,
        /// Signed release channel.
        #[arg(long, value_enum, default_value_t = ChannelArg::Stable)]
        channel: ChannelArg,
        /// Immutable release version; bypasses channel resolution.
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
        /// Explicitly acknowledge that a pinned version may downgrade the device.
        #[arg(long, requires = "version")]
        allow_downgrade: bool,
        /// Explicit serial port for ESP boards.
        #[arg(long, value_name = "PORT")]
        port: Option<String>,
        /// Provisioning behavior. Preserve is always the default.
        #[arg(long, value_enum, default_value_t = WifiMode::Preserve)]
        wifi: WifiMode,
        /// SSID used with `--wifi configure`.
        #[arg(long, value_name = "SSID")]
        wifi_ssid: Option<String>,
        /// Read the Wi-Fi password from standard input; it never appears in argv.
        #[arg(long)]
        wifi_password_stdin: bool,
        /// Read HOPSPOT_WIFI_SSID/PASSWORD from the environment.
        #[arg(long)]
        wifi_from_env: bool,
        #[arg(
            long,
            value_name = "TARGET",
            help = "One outbound Reticulum TCP target as IPv4, hostname, or URL; port defaults to 4242."
        )]
        tcp_client: Option<String>,
        /// Use only a previously verified local cache.
        #[arg(long)]
        offline: bool,
        /// Confirm the exact board noninteractively.
        #[arg(long)]
        yes: bool,
        /// Open a basic serial monitor after verified ESP flashing.
        #[arg(long, conflicts_with = "json")]
        monitor: bool,
        /// Emit newline-delimited schema-1 events and never prompt.
        #[arg(long)]
        json: bool,
        /// Build and use repository firmware instead of a signed public release.
        #[arg(long, hide = true)]
        local_build: bool,
        /// Use an extracted, signed release candidate for pre-publication qualification.
        #[arg(
            long,
            value_name = "DIR",
            hide = true,
            conflicts_with_all = ["version", "offline", "local_build"]
        )]
        candidate: Option<PathBuf>,
        /// Explicit mounted UF2 bootloader directory.
        #[arg(long, value_name = "DIR", hide = true)]
        mount: Option<PathBuf>,
    },
    /// Build sparse developer artifacts for one board.
    #[command(hide = true)]
    Build {
        board: String,
        #[arg(long, value_name = "DIR")]
        out_root: Option<PathBuf>,
        #[arg(long, value_name = "VERSION", hide = true)]
        developer_version: Option<String>,
    },
    /// Assemble manifest v2 after all shipping developer artifacts have been built.
    #[command(hide = true)]
    AssembleManifest {
        #[arg(long, value_name = "DIR")]
        out_root: PathBuf,
        #[arg(long, value_enum, default_value_t = ChannelArg::Preview)]
        channel: ChannelArg,
        #[arg(long, value_name = "COMMIT")]
        commit: String,
        #[arg(long, value_name = "KEY_ID")]
        key_id: String,
        #[arg(long, value_name = "VERSION", hide = true, requires = "boards")]
        developer_version: Option<String>,
        #[arg(
            long = "board",
            value_name = "BOARD",
            hide = true,
            requires = "developer_version"
        )]
        boards: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum CacheCommand {
    /// Verify an extracted signed candidate directory and import it atomically.
    Import {
        /// Extracted signed candidate directory.
        #[arg(value_name = "SIGNED_CANDIDATE_DIR")]
        candidate: PathBuf,
        /// Emit newline-delimited schema-1 events.
        #[arg(long)]
        json: bool,
    },
}

impl Cli {
    pub(crate) fn json_mode(&self) -> bool {
        match &self.command {
            Some(CommandMode::List { json })
            | Some(CommandMode::Doctor { json, .. })
            | Some(CommandMode::Flash { json, .. })
            | Some(CommandMode::Cache {
                command: CacheCommand::Import { json, .. },
            }) => *json,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum ChannelArg {
    #[default]
    Stable,
    Preview,
}

impl ChannelArg {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Preview => "preview",
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{error::ErrorKind, Parser};

    use super::{Cli, CommandMode};

    #[test]
    fn json_and_monitor_are_rejected_with_usage_exit_code() {
        let result = Cli::try_parse_from([
            "hopspot-flash",
            "flash",
            "heltec-v4",
            "--yes",
            "--json",
            "--monitor",
        ]);
        let error = match result {
            Ok(_) => panic!("JSON and raw serial output must not share stdout"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn json_and_monitor_remain_valid_individually() {
        assert!(
            Cli::try_parse_from(["hopspot-flash", "flash", "heltec-v4", "--yes", "--json",])
                .is_ok()
        );
        assert!(Cli::try_parse_from(
            ["hopspot-flash", "flash", "heltec-v4", "--yes", "--monitor",]
        )
        .is_ok());
    }

    #[test]
    fn cache_import_accepts_an_extracted_directory_and_json_mode() {
        assert!(Cli::try_parse_from([
            "hopspot-flash",
            "cache",
            "import",
            "/tmp/signed-candidate",
            "--json",
        ])
        .is_ok());
    }

    #[test]
    fn developer_manifest_inputs_require_each_other_and_repeat_boards() {
        let base = [
            "hopspot-flash",
            "assemble-manifest",
            "--out-root",
            "/tmp/candidate",
            "--commit",
            "0123456789abcdef0123456789abcdef01234567",
            "--key-id",
            "1FB2CA18B2C25E1F",
        ];
        let expected_developer_version = format!("{}-dev.clean.abc", env!("CARGO_PKG_VERSION"));
        let mut version_only = base.to_vec();
        version_only.extend(["--developer-version", expected_developer_version.as_str()]);
        assert!(Cli::try_parse_from(version_only).is_err());

        let mut board_only = base.to_vec();
        board_only.extend(["--board", "heltec-v4"]);
        assert!(Cli::try_parse_from(board_only).is_err());

        let mut complete = base.to_vec();
        complete.extend([
            "--developer-version",
            expected_developer_version.as_str(),
            "--board",
            "heltec-v4",
            "--board",
            "t-echo",
        ]);
        let cli = Cli::try_parse_from(complete).expect("valid local manifest inputs");
        let Some(CommandMode::AssembleManifest {
            developer_version,
            boards,
            ..
        }) = cli.command
        else {
            panic!("assemble-manifest command was not parsed");
        };
        assert_eq!(
            (developer_version.as_deref(), boards.as_slice()),
            (
                Some(expected_developer_version.as_str()),
                ["heltec-v4".to_string(), "t-echo".to_string()].as_slice()
            )
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum WifiMode {
    #[default]
    Preserve,
    Configure,
    Clear,
}
