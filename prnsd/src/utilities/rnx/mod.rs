mod args;
mod client;
mod error;
mod identity;
mod listener;

pub use args::RnxArgs;
pub use error::RnxError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnxOutcome {
    exit_code: u8,
}

impl RnxOutcome {
    pub const fn exit_code(self) -> u8 {
        self.exit_code
    }
}

pub async fn run(args: RnxArgs) -> Result<RnxOutcome, RnxError> {
    if args.version {
        println!(
            "prnsd x {} (RNS 1.3.8 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(RnxOutcome { exit_code: 0 });
    }
    if args.listen || args.print_identity {
        listener::run(args).await?;
        return Ok(RnxOutcome { exit_code: 0 });
    }
    let Some(destination) = args
        .destination
        .map(|destination| destination.destination())
    else {
        print!("{}", crate::cli::x_help());
        return Ok(RnxOutcome { exit_code: 0 });
    };
    if args.command.is_none() && !args.interactive {
        print!("{}", crate::cli::x_help());
        return Ok(RnxOutcome { exit_code: 0 });
    }
    client::run(args, destination).await
}
