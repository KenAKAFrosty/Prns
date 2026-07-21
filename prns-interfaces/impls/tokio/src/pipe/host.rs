use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, Join, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Owns the child with kill-on-drop so a closed or respawned pipe cannot orphan its subprocess.
pub struct PipeStream {
    #[allow(dead_code)]
    child: Child,
    io: Join<ChildStdout, ChildStdin>,
}

// `PipeStream` and every inner field are `Unpin`, so the poll methods can project with safe `Pin::new`.
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
