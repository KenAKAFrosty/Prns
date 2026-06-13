use clap::Parser;
use personal_rns::interfaces::ifac::DEFAULT_IFAC_SIZE;

#[derive(Parser)]
#[command(name = "personal-rnsd", version, about = "Personal Reticulum daemon")]
pub struct Cli {
    #[arg(value_name = "SERIAL_DEVICE", help = "USB-serial device to drive the engine over")]
    pub device: String,

    #[arg(long, value_name = "NAME", help = "IFAC network name (joins a private network)")]
    pub ifac_netname: Option<String>,

    #[arg(long, value_name = "KEY", help = "IFAC passphrase for the named network")]
    pub ifac_netkey: Option<String>,

    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = DEFAULT_IFAC_SIZE,
        help = "IFAC authentication tag size in bytes"
    )]
    pub ifac_size: usize,
}
