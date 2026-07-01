//! The Android USB conduit, async side. The JNI layer is sync — it pushes bytes the phone read
//! off the USB device in (`push_inbound`), drains the bytes the engine wants written out
//! (`pull_outbound`), and reports attach/detach (`set_connected`) — exactly the API `lib.rs`
//! already calls. The reactor is async, so a [`BridgeStream`] bridges the gap: an
//! `AsyncRead`/`AsyncWrite` over an inbound mpsc (so a read parks until the JNI pushes, no
//! busy-poll) and a shared outbound queue (the JNI pulls). The `UsbAutoHost` serves it like any
//! other stream — `open_stream` standing in for opening a port, the connected flag driving its
//! rescan.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

/// The handle the JNI layer holds (cheap-clone): it feeds inbound bytes, drains outbound, and
/// flags attach/detach. The engine holds a clone too, opening a [`BridgeStream`] over it for the
/// USB-auto host and reading `connected` for the host's port scan.
pub struct AndroidUsbBridge {
    inbound: Arc<Mutex<Option<UnboundedSender<Vec<u8>>>>>,
    pending_inbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
    outbound: Arc<Mutex<VecDeque<u8>>>,
    connected: Arc<AtomicBool>,
    rescan: Arc<Notify>,
}

const PENDING_INBOUND_CHUNKS: usize = 8;

impl Clone for AndroidUsbBridge {
    fn clone(&self) -> Self {
        Self {
            inbound: Arc::clone(&self.inbound),
            pending_inbound: Arc::clone(&self.pending_inbound),
            outbound: Arc::clone(&self.outbound),
            connected: Arc::clone(&self.connected),
            rescan: Arc::clone(&self.rescan),
        }
    }
}

impl AndroidUsbBridge {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inbound: Arc::new(Mutex::new(None)),
            pending_inbound: Arc::new(Mutex::new(VecDeque::new())),
            outbound: Arc::new(Mutex::new(VecDeque::new())),
            connected: Arc::new(AtomicBool::new(false)),
            rescan: Arc::new(Notify::new()),
        }
    }

    /// Report the USB device attaching or detaching, and poke the host to rescan immediately.
    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
        self.rescan.notify_one();
    }

    /// Feed bytes the phone read off the USB device to the current stream. If the Java/Kotlin side
    /// wins the startup race and reads before the USB-auto host has opened the bridge stream, keep a
    /// small bounded backlog so the initial handshake is not lost.
    pub fn push_inbound(&self, bytes: &[u8]) {
        if let Ok(guard) = self.inbound.lock() {
            if let Some(sender) = guard.as_ref() {
                let _ = sender.send(bytes.to_vec());
                return;
            }
        }
        if let Ok(mut pending) = self.pending_inbound.lock() {
            if pending.len() >= PENDING_INBOUND_CHUNKS {
                pending.pop_front();
            }
            pending.push_back(bytes.to_vec());
        }
    }

    /// Drain up to `out.len()` bytes the engine wants written to the USB device.
    pub fn pull_outbound(&self, out: &mut [u8]) -> usize {
        let Ok(mut queue) = self.outbound.lock() else {
            return 0;
        };
        let mut written = 0;
        for slot in out.iter_mut() {
            let Some(byte) = queue.pop_front() else {
                break;
            };
            *slot = byte;
            written += 1;
        }
        written
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// The host's rescan signal — poked by [`set_connected`](Self::set_connected) so a plug shows
    /// the instant the OS reports it.
    #[must_use]
    pub fn rescan(&self) -> Arc<Notify> {
        Arc::clone(&self.rescan)
    }

    /// Open a fresh stream for one connection: a new inbound channel (its sender replaces any
    /// prior one, so `push_inbound` reaches this stream) over the shared outbound queue.
    #[must_use]
    pub fn open_stream(&self) -> BridgeStream {
        let (tx, rx) = unbounded_channel::<Vec<u8>>();
        if let Ok(mut pending) = self.pending_inbound.lock() {
            while let Some(chunk) = pending.pop_front() {
                let _ = tx.send(chunk);
            }
        }
        if let Ok(mut guard) = self.inbound.lock() {
            *guard = Some(tx);
        }
        BridgeStream {
            rx,
            leftover: Vec::new(),
            pos: 0,
            outbound: Arc::clone(&self.outbound),
        }
    }
}

impl Default for AndroidUsbBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// One connection's async byte stream: inbound parks on the mpsc until the JNI pushes; outbound
/// is appended to the shared queue the JNI drains.
pub struct BridgeStream {
    rx: UnboundedReceiver<Vec<u8>>,
    leftover: Vec<u8>,
    pos: usize,
    outbound: Arc<Mutex<VecDeque<u8>>>,
}

impl AsyncRead for BridgeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos >= self.leftover.len() {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.leftover = chunk;
                    self.pos = 0;
                }
                // The sender dropped (the bridge opened a newer stream): end this one so the host
                // prunes it.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let available = self.leftover.len() - self.pos;
        let n = available.min(buf.remaining());
        let start = self.pos;
        buf.put_slice(&self.leftover[start..start + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for BridgeStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Ok(mut queue) = self.outbound.lock() {
            queue.extend(buf.iter().copied());
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn inbound_pushes_read_through_the_stream_and_writes_drain_to_the_bridge() {
        let bridge = AndroidUsbBridge::new();
        let mut stream = bridge.open_stream();

        bridge.push_inbound(&[9, 8, 7]);
        let mut buf = [0u8; 8];
        let n = stream.read(&mut buf).await.expect("reads the pushed bytes");
        assert_eq!(&buf[..n], &[9, 8, 7]);

        stream
            .write_all(&[1, 2, 3, 4])
            .await
            .expect("writes to the bridge");
        let mut out = [0u8; 8];
        assert_eq!(bridge.pull_outbound(&mut out), 4);
        assert_eq!(&out[..4], &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn inbound_pushed_before_open_is_delivered_to_the_next_stream() {
        let bridge = AndroidUsbBridge::new();
        bridge.push_inbound(&[4, 5, 6]);

        let mut stream = bridge.open_stream();
        let mut buf = [0u8; 8];
        let n = stream
            .read(&mut buf)
            .await
            .expect("reads the pre-open bytes");
        assert_eq!(&buf[..n], &[4, 5, 6]);
    }

    #[tokio::test]
    async fn a_chunk_larger_than_the_read_buffer_is_served_across_reads() {
        let bridge = AndroidUsbBridge::new();
        let mut stream = bridge.open_stream();
        bridge.push_inbound(&[1, 2, 3, 4, 5]);

        let mut small = [0u8; 2];
        assert_eq!(stream.read(&mut small).await.unwrap(), 2);
        assert_eq!(&small, &[1, 2]);
        assert_eq!(stream.read(&mut small).await.unwrap(), 2);
        assert_eq!(&small, &[3, 4]);
        assert_eq!(stream.read(&mut small).await.unwrap(), 1);
        assert_eq!(small[0], 5);
    }

    #[test]
    fn set_connected_flips_the_flag() {
        let bridge = AndroidUsbBridge::new();
        assert!(!bridge.is_connected());
        bridge.set_connected(true);
        assert!(bridge.is_connected());
    }
}
