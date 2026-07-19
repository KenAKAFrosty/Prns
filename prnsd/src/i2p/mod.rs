use std::process::ExitCode;

use crate::cli;

mod doctor;
mod setup;

pub async fn run(args: cli::I2pArgs) -> ExitCode {
    match args.command {
        cli::I2pCommand::Doctor(args) => {
            let remote_access = remote_sam_access(&args.sam);
            let request = doctor::I2pDoctorRequest::new(args.sam.sam_bridge, remote_access);
            match doctor::run(request).await {
                Ok(ready) => {
                    println!("{ready}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        cli::I2pCommand::Setup(args) => {
            let remote_access = remote_sam_access(&args.sam);
            let reachability = if args.connectable {
                setup::SetupReachability::Connectable
            } else {
                setup::SetupReachability::OutboundOnly
            };
            let browser = if args.open_guidance {
                setup::BrowserPreference::OpenApplicablePage
            } else {
                setup::BrowserPreference::PrintOnly
            };
            let request = match setup::I2pSetupRequest::new(
                args.sam.sam_bridge,
                remote_access,
                args.peer,
                reachability,
                browser,
            ) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let report = setup::run(request).await;
            println!("{report}");
            if report.is_ready() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn remote_sam_access(args: &cli::I2pSamArgs) -> doctor::RemoteSamAccess {
    if args.allow_remote_sam {
        doctor::RemoteSamAccess::ExplicitlyAllowed
    } else {
        doctor::RemoteSamAccess::LoopbackOnly
    }
}
