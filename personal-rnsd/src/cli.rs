use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(name = "personal-rnsd", version, about = "Personal Reticulum daemon")]
pub struct Cli {
    /// Reticulum config directory. Defaults to RNS's own search order (`/etc/reticulum`, then
    /// `~/.config/reticulum`, then `~/.reticulum`). The daemon reads `<dir>/config` and owns the
    /// node's identity at `<dir>/storage/transport_identity`.
    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}
