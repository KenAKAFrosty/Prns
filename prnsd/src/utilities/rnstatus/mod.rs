mod args;
mod discovery;
mod json;
mod remote;
mod render;

use args::RnstatusTarget;
pub use args::{RnstatusArgs, RnstatusSort};

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::configuration::LoadedConfiguration;

pub async fn run(args: RnstatusArgs) -> Result<(), String> {
    if args.version {
        println!(
            "prnsd status {} (RNS 1.4.0 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(|error| error.to_string())?;
    if args.verbose != 0 {
        for warning in &configuration.report.warnings {
            eprintln!("{warning}");
        }
    }
    if args.discovered || args.discovery_details {
        let output = discovery::render(&configuration.discovered.dir, &args)?;
        print!("{output}");
        return Ok(());
    }
    match args.target()? {
        RnstatusTarget::Local => run_local(&configuration, &args).await,
        RnstatusTarget::Remote {
            transport_identity,
            management_identity,
        } => {
            run_remote(
                &configuration,
                transport_identity,
                management_identity,
                &args,
            )
            .await
        }
    }
}

async fn run_local(configuration: &LoadedConfiguration, args: &RnstatusArgs) -> Result<(), String> {
    let client = configuration
        .local_rpc_client(args.remote_timeout.get())
        .map_err(|error| error.to_string())?;
    if args.monitor {
        return monitor(&client, args).await;
    }
    let output = query_and_render(&client, args).await?;
    print!("{output}");
    Ok(())
}

async fn run_remote(
    configuration: &LoadedConfiguration,
    transport_identity: personal_rns::identity::IdentityHash,
    management_identity: &std::path::Path,
    args: &RnstatusArgs,
) -> Result<(), String> {
    if args.monitor {
        loop {
            let started = Instant::now();
            let output = query_remote_and_render(
                configuration,
                transport_identity,
                management_identity,
                args,
            )
            .await
            .unwrap_or_else(|error| format!("prnsd status: {error}\n"));
            print!("\u{1b}[H\u{1b}[2J{output}");
            let wait = args
                .monitor_interval
                .get()
                .saturating_sub(started.elapsed())
                .max(Duration::from_millis(200));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                result = tokio::signal::ctrl_c() => {
                    return result.map_err(|error| format!("could not listen for Ctrl-C: {error}"));
                }
            }
        }
    }
    let output =
        query_remote_and_render(configuration, transport_identity, management_identity, args)
            .await?;
    print!("{output}");
    Ok(())
}

async fn monitor(
    client: &personal_rns::shared_instance::SharedInstanceRpcClient,
    args: &RnstatusArgs,
) -> Result<(), String> {
    loop {
        let started = Instant::now();
        let output = query_and_render(client, args)
            .await
            .unwrap_or_else(|error| format!("prnsd status: {error}\n"));
        print!("\u{1b}[H\u{1b}[2J{output}");
        let wait = args
            .monitor_interval
            .get()
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(200));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            result = tokio::signal::ctrl_c() => {
                return result.map_err(|error| format!("could not listen for Ctrl-C: {error}"));
            }
        }
    }
}

async fn query_and_render(
    client: &personal_rns::shared_instance::SharedInstanceRpcClient,
    args: &RnstatusArgs,
) -> Result<String, String> {
    let report = client
        .interface_stats()
        .await
        .map_err(|error| error.to_string())?;
    let link_count = if args.link_stats {
        Some(
            client
                .link_count()
                .await
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if args.json {
        json::render(&report).map_err(|error| format!("could not encode JSON status: {error}"))
    } else {
        Ok(render::human(
            &report,
            link_count,
            args,
            unix_time_seconds(),
        ))
    }
}

async fn query_remote_and_render(
    configuration: &LoadedConfiguration,
    transport_identity: personal_rns::identity::IdentityHash,
    management_identity: &std::path::Path,
    args: &RnstatusArgs,
) -> Result<String, String> {
    let report = remote::query(
        configuration,
        transport_identity,
        management_identity,
        args.link_stats,
        args.remote_timeout.get(),
    )
    .await
    .map_err(|error| error.to_string())?;
    if args.json {
        json::render(&report.status)
            .map_err(|error| format!("could not encode JSON status: {error}"))
    } else {
        Ok(render::human(
            &report.status,
            report.link_count,
            args,
            unix_time_seconds(),
        ))
    }
}

fn unix_time_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}
