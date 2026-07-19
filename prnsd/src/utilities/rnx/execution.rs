use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use personal_rns::rnx::{
    ExecutedCommand, ExecutionConclusion, ExecutionRequest, ExecutionResult,
    MAX_RETURNED_STREAM_BYTES,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[cfg(unix)]
use nix::sys::signal::{killpg, Signal};
#[cfg(unix)]
use nix::unistd::Pid;

pub async fn execute(request: ExecutionRequest) -> ExecutionResult {
    let started_at = unix_time();
    let Some(arguments) = shlex::split(&request.command) else {
        return ExecutionResult::NotExecuted { started_at };
    };
    let Some((program, arguments)) = arguments.split_first() else {
        return ExecutionResult::NotExecuted { started_at };
    };
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let Ok(mut child) = command.spawn() else {
        return ExecutionResult::NotExecuted { started_at };
    };
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin_task = tokio::spawn(async move {
        if let Some(mut writer) = stdin {
            if let Some(input) = request.stdin {
                let _ = writer.write_all(&input).await;
            }
            let _ = writer.shutdown().await;
        }
    });
    let stdout_task = tokio::spawn(capture(stdout, returned_limit(request.stdout_limit)));
    let stderr_task = tokio::spawn(capture(stderr, returned_limit(request.stderr_limit)));
    let (status, conclusion) = match request.timeout_seconds {
        Some(seconds) => {
            let timeout = Duration::from_secs_f64(seconds);
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(status) => (status.ok(), ExecutionConclusion::CompletedAt(unix_time())),
                Err(_) => {
                    terminate(&mut child);
                    let status = child.wait().await.ok();
                    (status, ExecutionConclusion::TimedOut)
                }
            }
        }
        None => (
            child.wait().await.ok(),
            ExecutionConclusion::CompletedAt(unix_time()),
        ),
    };
    let _ = stdin_task.await;
    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
    ExecutionResult::Executed(ExecutedCommand {
        return_code: status.and_then(return_code),
        stdout: stdout.returned,
        stderr: stderr.returned,
        total_stdout: stdout.total,
        total_stderr: stderr.total,
        started_at,
        conclusion,
    })
}

fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(group) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .map(Pid::from_raw)
    {
        let _ = killpg(group, Signal::SIGKILL);
        return;
    }
    let _ = child.start_kill();
}

#[derive(Default)]
struct CapturedStream {
    returned: Vec<u8>,
    total: u64,
}

async fn capture(reader: Option<impl AsyncRead + Unpin>, returned_limit: usize) -> CapturedStream {
    let Some(mut reader) = reader else {
        return CapturedStream::default();
    };
    let mut captured = CapturedStream {
        returned: Vec::with_capacity(returned_limit.min(64 * 1024)),
        total: 0,
    };
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let Ok(read) = reader.read(&mut buffer).await else {
            return captured;
        };
        if read == 0 {
            return captured;
        }
        captured.total = captured.total.saturating_add(read as u64);
        let remaining = returned_limit.saturating_sub(captured.returned.len());
        captured
            .returned
            .extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

fn returned_limit(requested: Option<u64>) -> usize {
    requested
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(MAX_RETURNED_STREAM_BYTES)
        .min(MAX_RETURNED_STREAM_BYTES)
}

fn unix_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn return_code(status: std::process::ExitStatus) -> Option<i32> {
    if let Some(code) = status.code() {
        return Some(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|signal| -signal)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_captures_limits_counts_stdin_and_return_code() {
        let result = execute(ExecutionRequest {
            command: String::from(
                "sh -c 'read value; printf %s-stdout \"$value\"; printf stderr >&2; exit 7'",
            ),
            timeout_seconds: Some(5.0),
            stdout_limit: Some(4),
            stderr_limit: Some(2),
            stdin: Some(b"input\n".to_vec()),
        })
        .await;
        let ExecutionResult::Executed(result) = result else {
            panic!("command was not executed");
        };
        assert_eq!(result.return_code, Some(7));
        assert_eq!(result.stdout, b"inpu");
        assert_eq!(result.stderr, b"st");
        assert_eq!(result.total_stdout, 12);
        assert_eq!(result.total_stderr, 6);
        assert!(matches!(
            result.conclusion,
            ExecutionConclusion::CompletedAt(_)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_timeout_kills_and_reaps_the_process() {
        let result = execute(ExecutionRequest {
            command: String::from("sh -c 'printf before; sleep 5; printf after'"),
            timeout_seconds: Some(0.05),
            stdout_limit: None,
            stderr_limit: None,
            stdin: None,
        })
        .await;
        let ExecutionResult::Executed(result) = result else {
            panic!("command was not executed");
        };
        assert_eq!(result.stdout, b"before");
        assert_eq!(result.total_stdout, 6);
        assert_eq!(result.conclusion, ExecutionConclusion::TimedOut);
    }
}
