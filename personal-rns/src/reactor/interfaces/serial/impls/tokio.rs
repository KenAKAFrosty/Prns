use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::interfaces::serial::core;

/// A serial interface that owns its medium's whole lifecycle: `open` yields a fresh async
/// byte stream (the consumer supplies it, e.g. a reopened `tokio_serial::SerialStream`), and
/// the interface reconnects on its own — serve a connection until it drops, wait `reconnect`,
/// reopen. A single never-dropping stream is just a factory that yields once.
pub struct SerialInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
}

impl<Open> SerialInterface<Open> {
    #[must_use]
    pub fn new(id: InterfaceId, open: Open, reconnect: Duration) -> Self {
        Self {
            id,
            open,
            reconnect,
        }
    }
}

impl<Open, Fut, S> Interface for SerialInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id)
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            if let Ok(stream) = (self.open)().await {
                serve(stream, &mut seam).await;
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam,
/// drain the seam and frame outbound onto the wire. Returns on any IO error so the caller can
/// reconnect; a fresh decoder per connection discards any half-frame the drop interrupted.
async fn serve<S, Seam>(mut stream: S, seam: &mut Seam)
where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let mut decoder = core::Decoder::new();
    let mut read_buf = [0u8; core::READ_BUF_LEN];

    loop {
        tokio::select! {
            read = stream.read(&mut read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                core::deframe_to_seam(&mut decoder, &read_buf[..read], seam).await;
            }
            outbound = seam.next_outbound() => {
                let mut frame_buf = [0u8; core::FRAMED_LEN];
                if let Some(framed) = core::frame_for_wire(outbound.bytes(), &mut frame_buf) {
                    if stream.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use crate::reactor::interface_seam::OutboundFrame;
    use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// queue the test fills — so the interface's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: UnboundedReceiver<OutboundFrame>,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> OutboundFrame {
            match self.outbound.recv().await {
                Some(frame) => frame,
                None => ::core::future::pending().await,
            }
        }
    }

    fn test_id() -> InterfaceId {
        InterfaceId::new([0xD0; 16])
    }

    #[tokio::test]
    async fn frames_outbound_and_deframes_inbound_across_a_real_async_stream() {
        // A duplex stands in for the serial wire: the factory yields its end once, then refuses
        // (the reconnect loop just retries harmlessly until the test drops the task).
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        tokio::spawn(SerialInterface::new(test_id(), open, Duration::from_millis(10)).run(seam));

        // Inbound: the test writes a framed payload (FLAG/ESC bytes exercise the escaping) onto
        // the wire; the interface deframes it and hands the original across the seam.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).expect("encodes the payload");
        test_wire
            .write_all(&framed[..n])
            .await
            .expect("writes onto the wire");

        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(
            received, payload,
            "the interface deframes inbound bytes for the seam"
        );

        // Outbound: the seam yields a frame; the interface frames it onto the wire; the test
        // reads it back and deframes to the original.
        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .send(OutboundFrame::new(&out_payload))
            .expect("the interface holds the outbound queue");

        let mut decoder = core::Decoder::new();
        let mut buf = [0u8; 64];
        let decoded = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let n = test_wire.read(&mut buf).await.expect("reads from the wire");
                for &byte in &buf[..n] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            return frame.to_vec();
                        }
                    }
                }
            }
        })
        .await
        .expect("the interface frames outbound within the window");
        assert_eq!(
            decoded, out_payload,
            "the interface frames outbound packets onto the wire"
        );
    }
}
