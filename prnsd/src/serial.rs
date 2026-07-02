//! Host serial transport for the serial-family interfaces (serial, KISS, AX.25-KISS, RNode). The
//! interfaces are generic over their byte stream, so this is the one seam that knows how a serial
//! port is opened on each platform.
//!
//! Off Windows this is simply tokio-serial's async stream. On Windows it is a blocking-open + threaded
//! bridge instead, because mio-serial's async `open_native_async` is unreliable with ESP32-native-USB
//! RNodes: it opens the port, closes it, and re-opens in overlapped mode, and that second open races
//! the USB re-enumeration such a device performs when the open toggles DTR — so reads surface as
//! phantom EOFs and the link never settles. serialport's blocking `open()` does a single `CreateFile`
//! (and never reads the port settings back, so it also tolerates the 1.5-stop-bit CDC line coding),
//! which opens these devices reliably. We open once on a worker thread and bridge its blocking
//! reads/writes to the reactor over channels.

use std::io;

#[cfg(not(windows))]
use tokio_serial::SerialPortBuilderExt;

/// The concrete serial stream the host interfaces run over: tokio-serial's async stream off Windows,
/// and the blocking-bridge stream on Windows.
#[cfg(not(windows))]
pub type HostSerial = tokio_serial::SerialStream;
#[cfg(windows)]
pub type HostSerial = windows_bridge::ThreadedSerial;

/// Open `path` at `baud` (8N1) as an async stream for a serial-family interface, using the reliable
/// transport for the platform. Called by each serial-family interface's open factory.
#[cfg(not(windows))]
pub fn open_host_serial(path: &str, baud: u32) -> io::Result<HostSerial> {
    tokio_serial::new(path, baud)
        .open_native_async()
        .map_err(io::Error::other)
}

#[cfg(windows)]
pub fn open_host_serial(path: &str, baud: u32) -> io::Result<HostSerial> {
    windows_bridge::open(path, baud)
}

#[cfg(windows)]
mod windows_bridge {
    use std::io::{self, Read, Write};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::mpsc;
    use tokio_serial::SerialPort;

    /// How long a blocking read waits for data before returning so the I/O thread can service pending
    /// writes and notice a closed channel. serialport reports an idle read as `TimedOut`.
    const READ_POLL: Duration = Duration::from_millis(20);
    /// The blocking read scratch size.
    const READ_CHUNK: usize = 512;

    /// A blocking serial port driven on one worker thread and presented to the reactor as an async
    /// stream. The thread interleaves reads and writes on a single handle: each pass it drains any
    /// queued writes, then does one bounded blocking read. Received bytes arrive over `inbound`;
    /// outbound bytes are queued on `outbound`.
    ///
    /// Both channels are unbounded *on purpose*. A bounded inbound channel is wrong here: during the
    /// bring-up settle the reactor is not reading yet, so a bounded channel fills with the device's
    /// idle telemetry and then either blocks the I/O thread — starving the detect/config writes it
    /// must still service — or drops chunks, losing the config echoes the validation read-back needs.
    /// Unbounded never blocks and never drops; the reactor drains it continuously, so it stays small.
    pub struct ThreadedSerial {
        inbound: mpsc::UnboundedReceiver<io::Result<Vec<u8>>>,
        outbound: mpsc::UnboundedSender<Vec<u8>>,
        chunk: Vec<u8>,
        offset: usize,
        eof: bool,
    }

    /// Open `path` at `baud` with serialport's blocking `open` — a single `CreateFile`, no settings
    /// read-back, no overlapped re-open — and spawn the I/O thread that bridges it to the channels.
    pub fn open(path: &str, baud: u32) -> io::Result<ThreadedSerial> {
        let port = tokio_serial::new(path, baud)
            .timeout(READ_POLL)
            .open()
            .map_err(io::Error::other)?;
        let (in_tx, in_rx) = mpsc::unbounded_channel::<io::Result<Vec<u8>>>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || io_loop(port, in_tx, out_rx));
        Ok(ThreadedSerial {
            inbound: in_rx,
            outbound: out_tx,
            chunk: Vec::new(),
            offset: 0,
            eof: false,
        })
    }

    /// The single I/O thread: drain queued writes, then one blocking read, forever — until either
    /// channel closes (the reactor dropped the stream) or the port errors.
    fn io_loop(
        mut port: Box<dyn SerialPort>,
        in_tx: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
        mut out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            // Service every pending write before the next read so outbound is never starved.
            loop {
                match out_rx.try_recv() {
                    Ok(data) => {
                        if port.write_all(&data).is_err() {
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

    impl AsyncRead for ThreadedSerial {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            // Deliver any bytes left over from the previous chunk first.
            if this.offset < this.chunk.len() {
                let take = (this.chunk.len() - this.offset).min(buf.remaining());
                buf.put_slice(&this.chunk[this.offset..this.offset + take]);
                this.offset += take;
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                return Poll::Ready(Ok(())); // a 0-byte read signals EOF
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
                    Poll::Ready(Ok(())) // the I/O thread ended — end of stream
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl AsyncWrite for ThreadedSerial {
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
            Poll::Ready(Ok(())) // the I/O thread flushes after each write
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
