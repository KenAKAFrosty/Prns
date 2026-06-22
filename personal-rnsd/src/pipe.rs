//! The subprocess transport for the pipe interface: spawn the configured command and hand the
//! reactor a single async byte stream over its stdout/stdin. This is the daemon's side of
//! `PipeInterface`, exactly as `tokio_serial` is for the serial interface — the core library owns
//! the protocol and the framing, the daemon owns the OS pipe.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, Join, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A spawned subprocess presented as one `AsyncRead + AsyncWrite` stream: reads come from the
/// child's stdout, writes go to its stdin. It owns the [`Child`] with kill-on-drop, so when the
/// interface's serve loop drops the stream (the pipe closed, or a respawn), the subprocess is killed
/// rather than left orphaned — matching RNS, which `kill()`s the process when its pipe ends.
pub struct PipeStream {
    /// Held solely to keep the subprocess alive and kill it on drop; never read directly.
    #[allow(dead_code)]
    child: Child,
    io: Join<ChildStdout, ChildStdin>,
}

// `PipeStream` is `Unpin` — `Child`, `ChildStdout`, `ChildStdin`, and `Join` are all `Unpin` — so the
// poll methods project to the inner `io` with a safe `Pin::new`, honoring the crate's
// `forbid(unsafe_code)`.
impl AsyncRead for PipeStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, buf)
    }
}

impl AsyncWrite for PipeStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

/// Spawn `argv` (program followed by its arguments) with piped stdin/stdout and present it as a
/// [`PipeStream`]. The `PipeInterface` open factory calls this once per connection; a spawn failure
/// is an `io::Error` the interface treats as a closed pipe and retries after its respawn delay.
pub async fn spawn(argv: &[String]) -> io::Result<PipeStream> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty pipe command"))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("subprocess has no stdout pipe"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("subprocess has no stdin pipe"))?;
    Ok(PipeStream {
        child,
        io: tokio::io::join(stdout, stdin),
    })
}
