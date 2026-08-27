use std::io::Write;
use std::time::Duration;

use personal_rns::engine::RequestResponseTimeout;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::rnx::{
    decode_execution_result, encode_execution_request, ExecutionConclusion, ExecutionRequest,
    ExecutionResult, APP_NAME, COMMAND_PATH, EXECUTE_ASPECT,
};
use personal_rns::routing::announce::derive_single_destination_hash;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::units::DurationMillis;
use personal_rns::wire::DestinationHash;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::utilities::configuration::LoadedConfiguration;
use crate::utilities::session::{UtilityNodeClient, UtilityNodeIdentity, UtilityNodeSession};

use super::identity::load_identity;
use super::{RnxArgs, RnxError, RnxOutcome};

const REMOTE_EXECUTION_GRACE: Duration = Duration::from_secs(2);
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) async fn run(
    mut args: RnxArgs,
    destination: DestinationHash,
) -> Result<RnxOutcome, RnxError> {
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RnxError::Configuration)?;
    let secret = load_identity(&configuration, args.identity.as_deref())?;
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash();
    let execution_timeout = args
        .timeout
        .map_or(DEFAULT_EXECUTION_TIMEOUT, |timeout| timeout.get());
    let session = UtilityNodeSession::connect(
        &configuration,
        UtilityNodeIdentity::Private(secret),
        execution_timeout,
    )
    .await
    .map_err(RnxError::Session)?;
    session
        .run(move |client| async move {
            let path_timeout = match args.timeout {
                Some(timeout) => timeout.get(),
                None => client
                    .adaptive_path_timeout()
                    .await
                    .map_err(RnxError::Path)?,
            };
            client
                .ensure_path(destination, path_timeout)
                .await
                .map_err(RnxError::Path)?;
            verify_destination(&client, destination).await?;
            let establish = client.handle().establish_link_with_rtt(destination);
            let established = match args.timeout {
                Some(timeout) => tokio::time::timeout(timeout.get(), establish)
                    .await
                    .map_err(|_| RnxError::LinkTimeout(timeout.get()))?
                    .map_err(RnxError::Link)?,
                None => establish.await.map_err(RnxError::Link)?,
            };
            let link = established.link_id;
            if !args.no_id {
                client
                    .handle()
                    .identify(link, identity)
                    .await
                    .map_err(RnxError::Identify)?;
            }
            let mut last_code = None;
            if let Some(command) = args.command.take() {
                match execute_remote(&client, established, &args, command).await {
                    Ok(code) => last_code = code,
                    Err(error) if args.interactive => eprintln!("prnsd x: {error}"),
                    Err(error) => return Err(error),
                }
            }
            if args.interactive {
                interactive(&client, established, &args, &mut last_code).await?;
                client.handle().close_link(link);
                return Ok(RnxOutcome { exit_code: 0 });
            }
            client.handle().close_link(link);
            let exit_code = if args.mirror {
                last_code
                    .map(|code| code.rem_euclid(256) as u8)
                    .unwrap_or(240)
            } else {
                0
            };
            Ok(RnxOutcome { exit_code })
        })
        .await
        .map_err(RnxError::NodeStopped)?
}

async fn verify_destination(
    client: &UtilityNodeClient,
    destination: DestinationHash,
) -> Result<(), RnxError> {
    let identity = client
        .handle()
        .destination_identity_hash(destination)
        .await
        .ok_or(RnxError::IdentityUnavailable(destination))?;
    let expected = derive_single_destination_hash(&identity, APP_NAME, &[EXECUTE_ASPECT])
        .map_err(RnxError::Destination)?;
    if expected == destination {
        Ok(())
    } else {
        Err(RnxError::DestinationNameMismatch)
    }
}

async fn execute_remote(
    client: &UtilityNodeClient,
    established: personal_rns::engine::LinkEstablished,
    args: &RnxArgs,
    command: String,
) -> Result<Option<i32>, RnxError> {
    let request = encode_execution_request(&ExecutionRequest {
        command,
        timeout_seconds: Some(execution_timeout(args).as_secs_f64()),
        stdout_limit: args.stdout,
        stderr_limit: args.stderr,
        stdin: args.stdin.as_ref().map(|stdin| stdin.as_bytes().to_vec()),
    })
    .map_err(RnxError::Encode)?;
    let link_rtt = Duration::from_millis(established.rtt_millis).saturating_mul(4);
    let response_window = execution_timeout(args)
        .saturating_add(link_rtt)
        .saturating_add(REMOTE_EXECUTION_GRACE);
    let response_timeout =
        RequestResponseTimeout::Exact(DurationMillis::from_duration_saturating(response_window));
    let request_future = client.handle().request_with_response_timeout(
        established.link_id,
        RequestPathHash::of(COMMAND_PATH),
        &request,
        response_timeout,
    );
    let response = match args.result_timeout {
        Some(result_timeout) => {
            let total_window = response_window.saturating_add(result_timeout.get());
            tokio::time::timeout(total_window, request_future)
                .await
                .map_err(|_| RnxError::ResponseTimeout(total_window))?
                .map_err(RnxError::Request)?
        }
        None => request_future.await.map_err(RnxError::Request)?,
    };
    let result = decode_execution_result(&response.0).map_err(RnxError::ResponseCodec)?;
    print_result(&result, args.detailed)?;
    match result {
        ExecutionResult::NotExecuted { .. } => Err(RnxError::RemoteCouldNotExecute),
        ExecutionResult::Executed(executed) => Ok(executed.return_code),
    }
}

fn execution_timeout(args: &RnxArgs) -> Duration {
    execution_timeout_for(args.timeout.map(|timeout| timeout.get()))
}

fn execution_timeout_for(explicit: Option<Duration>) -> Duration {
    explicit.unwrap_or(DEFAULT_EXECUTION_TIMEOUT)
}

fn print_result(result: &ExecutionResult, detailed: bool) -> Result<(), RnxError> {
    let ExecutionResult::Executed(executed) = result else {
        return Ok(());
    };
    std::io::stdout()
        .write_all(&executed.stdout)
        .map_err(RnxError::Stdout)?;
    std::io::stderr()
        .write_all(&executed.stderr)
        .map_err(RnxError::Stderr)?;
    std::io::stdout().flush().map_err(RnxError::Stdout)?;
    std::io::stderr().flush().map_err(RnxError::Stderr)?;
    if detailed {
        println!("\n--- End of remote output, x done ---");
        if let ExecutionConclusion::CompletedAt(concluded_at) = executed.conclusion {
            println!(
                "Remote command execution took {:.3} seconds",
                (concluded_at - executed.started_at).max(0.0)
            );
        }
        println!(
            "Remote wrote {} bytes to stdout{}",
            executed.total_stdout,
            displayed_suffix(executed.stdout.len(), executed.total_stdout)
        );
        println!(
            "Remote wrote {} bytes to stderr{}",
            executed.total_stderr,
            displayed_suffix(executed.stderr.len(), executed.total_stderr)
        );
    } else if executed.stdout.len() as u64 != executed.total_stdout
        || executed.stderr.len() as u64 != executed.total_stderr
    {
        println!("\nOutput truncated before being returned:");
        if executed.stdout.len() as u64 != executed.total_stdout {
            println!("  stdout truncated to {} bytes", executed.stdout.len());
        }
        if executed.stderr.len() as u64 != executed.total_stderr {
            println!("  stderr truncated to {} bytes", executed.stderr.len());
        }
    }
    Ok(())
}

fn displayed_suffix(displayed: usize, total: u64) -> String {
    if displayed as u64 == total {
        String::new()
    } else {
        format!(", {displayed} bytes displayed")
    }
}

async fn interactive(
    client: &UtilityNodeClient,
    established: personal_rns::engine::LinkEstablished,
    args: &RnxArgs,
    last_code: &mut Option<i32>,
) -> Result<(), RnxError> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        match last_code.filter(|code| *code != 0) {
            Some(code) => print!("{code}> "),
            None => print!("> "),
        }
        std::io::stdout().flush().map_err(RnxError::Stdout)?;
        let Some(command) = lines.next_line().await.map_err(RnxError::Stdin)? else {
            return Ok(());
        };
        match command.to_lowercase().as_str() {
            "exit" | "quit" => return Ok(()),
            "clear" => {
                print!("\x1bc");
                std::io::stdout().flush().map_err(RnxError::Stdout)?;
            }
            _ => match execute_remote(client, established, args, command).await {
                Ok(code) => *last_code = code,
                Err(error) => eprintln!("prnsd x: {error}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_discovery_does_not_expand_the_remote_execution_budget() {
        assert_eq!(execution_timeout_for(None), Duration::from_secs(15));
        assert_eq!(
            execution_timeout_for(Some(Duration::from_millis(2_500))),
            Duration::from_millis(2_500)
        );
    }
}
