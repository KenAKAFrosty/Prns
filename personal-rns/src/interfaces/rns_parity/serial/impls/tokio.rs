use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::interfaces::framed_stream;
use crate::interfaces::rns_parity::serial::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// A serial interface that owns its medium's whole lifecycle: `open` yields a fresh async
/// byte stream (the consumer supplies it, e.g. a reopened `tokio_serial::SerialStream`), and
/// the interface reconnects on its own — serve a connection until it drops, wait `reconnect`,
/// reopen. A single never-dropping stream is just a factory that yields once.
pub struct SerialInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
    medium_id: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> SerialInterface<Open> {
    /// `medium_id` names *which* serial device this is — the port name or a stable device id the
    /// caller knows (the `open` closure that yields the stream hides it from us). Two distinct
    /// serial channels must pass distinct bytes; the same device across a reopen should pass the
    /// same, so its routes survive the reconnect.
    #[must_use]
    pub fn new(open: Open, reconnect: Duration, medium_id: &[u8]) -> Self {
        let medium_id = medium_id.to_vec();
        let id = InterfaceId::from_medium(InterfaceKind::Serial, &medium_id);
        Self {
            id,
            open,
            reconnect,
            medium_id,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id, derived from its device `medium_id`, for the app that wants to name it
    /// (an [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this interface's live-status handle for the app to read on its own render
    /// cadence. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<Open, Fut, S> Interface for SerialInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = super::super::core::SERIAL_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Serial;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
    }

    fn medium_id(&self) -> &[u8] {
        &self.medium_id
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let bitrate_bps = core::descriptor(self.id).bitrate_bps;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            if let Ok(stream) = (self.open)().await {
                self.status.set_connection(ConnectionState::Connected);
                framed_stream::serve::<
                    { core::READ_BUF_LEN },
                    { core::SERIAL_FRAME_LEN },
                    { core::FRAMED_LEN },
                    _,
                    _,
                >(
                    stream,
                    &mut seam,
                    &self.status,
                    &mut airtime,
                    &mut throughput,
                    bitrate_bps,
                    started,
                )
                .await;
                self.status.set_connection(ConnectionState::Disconnected);
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use crate::interfaces::InterfaceStatus;
    use crate::reactor::grant::{GrantConsumer, GrantProducer};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// grant lane the test fills — so the interface's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer<{ core::SERIAL_FRAME_LEN }>,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
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
        let (mut out_tx, out_rx) = tokio_grant_lane::<{ core::SERIAL_FRAME_LEN }>(2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        let interface = SerialInterface::new(open, Duration::from_millis(10), b"test-serial");
        let status = interface.status();
        tokio::spawn(interface.run(seam));

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
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();

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

        // The interface's live status reflects what crossed — readable by the app directly,
        // never through the engine. `serve` updates it concurrently, so poll to the window.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.connection() == ConnectionState::Connected
                    && status.rx_bytes() > 0
                    && status.tx_bytes() > 0
                    && status.airtime().is_some()
                    && status.transfer_rates().is_some()
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the live status reflects the connection + bytes both ways within the window");
    }
}
