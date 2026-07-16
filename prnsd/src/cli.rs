use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    #[default]
    Human,
    Json,
}

#[derive(Parser)]
#[command(name = "prnsd", version, about = "Personal Reticulum daemon")]
pub struct Cli {
    #[arg(long, value_enum, default_value_t)]
    pub log_format: LogFormat,

    /// Reticulum config directory. Defaults to RNS's own search order (`/etc/reticulum`, then
    /// `~/.config/reticulum`, then `~/.reticulum`). The daemon reads `<dir>/config` and owns the
    /// node's identity at `<dir>/storage/transport_identity`.
    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,

    #[arg(long, hide = true)]
    pub managed: bool,

    #[arg(long, hide = true)]
    pub print_banner: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_output_is_the_default() {
        let cli = Cli::try_parse_from(["prnsd"]).unwrap();
        assert_eq!(cli.log_format, LogFormat::Human);
        assert!(!cli.managed);
        assert!(!cli.print_banner);
    }

    #[test]
    fn json_output_is_explicit() {
        let cli = Cli::try_parse_from(["prnsd", "--log-format", "json"]).unwrap();
        assert_eq!(cli.log_format, LogFormat::Json);
    }

    #[test]
    fn launcher_modes_are_typed_and_hidden() {
        let cli = Cli::try_parse_from(["prnsd", "--managed", "--print-banner"]).unwrap();
        assert!(cli.managed);
        assert!(cli.print_banner);
    }
}
