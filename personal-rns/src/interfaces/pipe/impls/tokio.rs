use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::interfaces::framed_stream::{self, HdlcFraming};
use crate::interfaces::pipe::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// A pipe interface that owns its subprocess pipe's whole lifecycle (RNS `PipeInterface`): `open`
/// yields a fresh async byte stream — the consumer supplies it, e.g. the daemon spawning the
/// configured command and joining its stdout/stdin — and the interface respawns on its own: serve a
/// connection until the stream drops (the subprocess exits), wait `respawn`, open again. Structurally
/// the serial interface over a different medium; it frames with the same RNS HDLC octet-stuffing.
pub struct PipeInterface<Open> {
    id: InterfaceId,
    open: Open,
    respawn: Duration,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> PipeInterface<Open> {
    /// `channel_tag` names *which* pipe this is — the command line the caller spawns. Two distinct
    /// pipes must pass distinct bytes; the same command across a respawn should pass the same, so its
    /// routes survive the respawn.
    #[must_use]
    pub fn new(open: Open, respawn: Duration, channel_tag: &[u8]) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Pipe, &channel_tag);
        Self {
            id,
            open,
            respawn,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id, derived from its command `channel_tag`.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this interface's live-status handle for the app to read on its own render cadence.
    /// Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<Open, Fut, S> Interface for PipeInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = core::PIPE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Pipe;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let bitrate_bps = core::descriptor(self.id).bitrate_bps;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                HdlcFraming,
                { core::READ_BUF_LEN },
                { core::PIPE_FRAME_LEN },
                { core::FRAMED_LEN },
            >,
        > = None;
        loop {
            if let Ok(stream) = (self.open)().await {
                self.status.set_connection(ConnectionState::Connected);
                framed_stream::serve::<
                    HdlcFraming,
                    { core::READ_BUF_LEN },
                    { core::PIPE_FRAME_LEN },
                    { core::FRAMED_LEN },
                    _,
                    _,
                >(
                    stream,
                    buffers.get_or_insert_with(framed_stream::FramedBuffers::new),
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
            tokio::time::sleep(self.respawn).await;
        }
    }
}

impl<Open> crate::interfaces::ReportsStatus for PipeInterface<Open> {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use crate::interfaces::InterfaceStatus;
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{self, UnboundedSender};

    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer,
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
    async fn frames_outbound_and_deframes_inbound_over_a_subprocess_like_stream() {
        // A duplex stands in for the subprocess's stdout/stdin: the factory yields its end once,
        // then refuses (the respawn loop just retries harmlessly until the test drops the task).
        let (interface_wire, mut test_wire) = tokio::io::duplex(2048);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::PIPE_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        let interface = PipeInterface::new(open, Duration::from_millis(10), b"cat");
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        // Inbound: a framed payload (FLAG/ESC exercise escaping) crosses the pipe and lands deframed.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).expect("encodes the payload");
        test_wire
            .write_all(&framed[..n])
            .await
            .expect("writes onto the pipe");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload);

        // Outbound: the seam yields a frame; it leaves the pipe framed.
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
                let n = test_wire.read(&mut buf).await.expect("reads from the pipe");
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
        assert_eq!(decoded, out_payload);

        // Live status reflects the connection and bytes both ways.
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
