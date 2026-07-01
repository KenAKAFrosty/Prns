use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

const READ_POLL: Duration = Duration::from_millis(20);
const READ_CHUNK: usize = 512;

pub struct HostSerial {
    inbound: mpsc::UnboundedReceiver<io::Result<Vec<u8>>>,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    chunk: Vec<u8>,
    offset: usize,
    eof: bool,
}

pub fn open_host_serial(path: &str, baud: u32) -> io::Result<HostSerial> {
    let port = serialport::new(path, baud)
        .timeout(READ_POLL)
        .open()
        .map_err(io::Error::other)?;
    let (in_tx, in_rx) = mpsc::unbounded_channel::<io::Result<Vec<u8>>>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let thread_name = format!("hopspot-serial-{path}");
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || io_loop(port, in_tx, out_rx));
    Ok(HostSerial {
        inbound: in_rx,
        outbound: out_tx,
        chunk: Vec::new(),
        offset: 0,
        eof: false,
    })
}

fn io_loop(
    mut port: Box<dyn serialport::SerialPort>,
    in_tx: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    mut out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        loop {
            match out_rx.try_recv() {
                Ok(data) => {
                    if let Err(error) = port.write_all(&data) {
                        let _ = in_tx.send(Err(error));
                        return;
                    }
                    let _ = port.flush();
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => return,
            }
        }

        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                if in_tx.send(Ok(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                let _ = in_tx.send(Err(error));
                return;
            }
        }
    }
}

impl AsyncRead for HostSerial {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.offset < this.chunk.len() {
            let take = (this.chunk.len() - this.offset).min(buf.remaining());
            buf.put_slice(&this.chunk[this.offset..this.offset + take]);
            this.offset += take;
            return Poll::Ready(Ok(()));
        }
        if this.eof {
            return Poll::Ready(Ok(()));
        }
        match this.inbound.poll_recv(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.chunk = chunk;
                this.offset = 0;
                let take = this.chunk.len().min(buf.remaining());
                buf.put_slice(&this.chunk[..take]);
                this.offset = take;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Err(error)),
            Poll::Ready(None) => {
                this.eof = true;
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for HostSerial {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().outbound.send(buf.to_vec()) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
