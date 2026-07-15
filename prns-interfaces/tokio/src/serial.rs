use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::framed_stream;
use prns_core::interfaces::serial::core;
use prns_core::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind};
use prns_core::reactor::airtime::AirtimeLedger;
use prns_core::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;

/// A serial interface that owns its medium's whole lifecycle: `open` yields a fresh async
/// byte stream (the consumer supplies it, e.g. a reopened `tokio_serial::SerialStream`), and
/// the interface reconnects on its own — serve a connection until it drops, wait `reconnect`,
/// reopen. A single never-dropping stream is just a factory that yields once.
pub struct SerialInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> SerialInterface<Open> {
    /// `channel_tag` names *which* serial device this is — the port name or a stable
    /// device id the caller knows (the `open` closure that yields the stream hides it from
    /// us). Two distinct serial channels must pass distinct bytes; the same device across a
    /// reopen should pass the same, so its routes survive the reconnect.
    #[must_use]
    pub fn new(open: Open, reconnect: Duration, channel_tag: &[u8]) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Serial, &channel_tag);
        Self {
            id,
            open,
            reconnect,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id, derived from its device `channel_tag`, for the app that wants
    /// to name it (an [`AnnounceTarget::Interface`](prns_core::engine::AnnounceTarget), a log line).
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
    const HW_MTU: usize = prns_core::interfaces::serial::core::SERIAL_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Serial;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let bitrate = core::descriptor(self.id).bitrate;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                framed_stream::HdlcFraming,
                { core::READ_BUF_LEN },
                { core::FRAMED_LEN },
            >,
        > = None;
        loop {
            if let Ok(stream) = (self.open)().await {
                self.status.set_connection(ConnectionState::Connected);
                framed_stream::serve::<
                    framed_stream::HdlcFraming,
                    { core::READ_BUF_LEN },
                    { core::FRAMED_LEN },
                    _,
                    _,
                >(
                    stream,
                    buffers.get_or_insert_with(framed_stream::FramedBuffers::new),
                    &mut seam,
                    &mut framed_stream::WireMeters {
                        status: &self.status,
                        airtime: &mut airtime,
                        throughput: &mut throughput,
                        bitrate,
                        started,
                    },
                )
                .await;
                self.status.set_connection(ConnectionState::Disconnected);
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

impl<Open> prns_core::interfaces::ReportsStatus for SerialInterface<Open> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use prns_core::interfaces::InterfaceStatus;
    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// grant lane the test fills — so the interface's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if !self.sink.is_empty() {
                let _ = self.inbound.send(std::mem::take(&mut self.sink));
            }
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
        let (mut out_tx, out_rx) = tokio_grant_lane(core::SERIAL_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
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
