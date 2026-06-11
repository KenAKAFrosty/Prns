//! The tokio host side of the plug-and-play USB-auto interface: a hub that discovers CDC ports,
//! handshakes each, and multiplexes them all behind one [`InterfaceId`]. Where the legacy host
//! drove every port from one `mio` poll loop on a fixed cadence, here each port is its own async
//! task — it sleeps on its wire, wakes the instant a byte lands, and funnels straight into the
//! reactor's inbound — so the discovery latency and the poll-interval jitter both fall away.
//! Discovery itself is event-driven too: the consumer pokes a rescan signal the instant the OS
//! reports a hot-plug, so a board appears the moment it is plugged, not on the next poll.
//!
//! Inbound fans IN: every confirmed port fills its own grant lane and announces the commit on
//! the hub's port-notify funnel; the hub drains the named lane across the seam — the reactor's
//! own id-funnel pattern, one level down. Outbound fans OUT: the run loop borrows each frame
//! from the seam and write-grants it into every confirmed port's lane — the hub repeat the
//! `host_descriptor`'s `SameInterfaceRepeat` capability already accounts for.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId};
use crate::reactor::grant::{GrantConsumer, GrantProducer};
use crate::reactor::impls::tokio_reactor::{
    tokio_grant_lane, TokioGrantConsumer, TokioGrantProducer, TokioInterfaceStatus,
};
use crate::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use crate::reactor::interfaces::usb_auto::core::{
    self, Capabilities, HostInbound, Message, NodeTag,
};

/// A slow fallback re-enumeration. Hot-plug is event-driven (the consumer pokes the rescan
/// signal the instant the OS reports a change), so this only backstops a missed event, a host
/// with no hot-plug source (e.g. macOS), and re-opening a port whose task died without an unplug.
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(1);
/// How often a not-yet-confirmed port re-sends its `Hello` — covering a board that was still
/// booting when first opened, with no replug needed.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);

struct Port {
    id: String,
    key: u64,
    confirmed: bool,
    outbound: TokioGrantProducer<MAX_WIRE_FRAME_LEN>,
    inbound: TokioGrantConsumer<MAX_WIRE_FRAME_LEN>,
    task: JoinHandle<()>,
}

enum PortEvent {
    Confirmed { id: String },
    Closed { id: String },
}

#[derive(Clone)]
struct PortContext {
    node_tag: NodeTag,
    status: TokioInterfaceStatus,
    events: UnboundedSender<PortEvent>,
}

/// How many frames a port's lane holds in each direction before the hub (inbound) or
/// the wire (outbound) is behind: backpressure for the port's own reads, drop-on-full
/// for the broadcast fan-out, mirroring the reactor's egress posture.
const PORT_LANE_DEPTH: usize = 8;

pub struct UsbAutoHost<Scan, Open> {
    id: InterfaceId,
    node_tag: NodeTag,
    scan: Scan,
    open: Open,
    status: TokioInterfaceStatus,
    rescan: Arc<Notify>,
}

impl<Scan, Open> UsbAutoHost<Scan, Open> {
    #[must_use]
    pub fn new(id: InterfaceId, scan: Scan, open: Open, rescan: Arc<Notify>) -> Self {
        Self {
            id,
            node_tag: core::node_tag_for(id),
            scan,
            open,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
            rescan,
        }
    }

    /// A clone of the live-status handle for the app to read on its own render cadence. Call
    /// before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }

    fn refresh_connection(&self, ports: &[Port]) {
        let connection = if ports.iter().any(|port| port.confirmed) {
            ConnectionState::Connected
        } else if ports.is_empty() {
            ConnectionState::Disconnected
        } else {
            ConnectionState::Reconnecting
        };
        self.status.set_connection(connection);
    }
}

impl<Scan, Open, Fut, S> Interface for UsbAutoHost<Scan, Open>
where
    Scan: FnMut() -> Vec<String> + Send + 'static,
    Open: FnMut(String) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    const HW_MTU: usize = crate::interfaces::impls::usb_auto::core::HOST_USB_HW_MTU;

    fn descriptor(&self) -> InterfaceConfig {
        core::host_descriptor(self.id)
    }

    /// Multiplex every discovered, confirmed port behind this one interface: drain each port's
    /// inbound lane across the seam as its notify names it, broadcast every seam outbound to all
    /// of them, and re-enumerate ports whenever `rescan` is poked (the consumer's hot-plug
    /// signal) or the fallback timer fires.
    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel::<PortEvent>();
        let (port_notify_tx, mut port_notify_rx) = mpsc::unbounded_channel::<u64>();
        let context = PortContext {
            node_tag: self.node_tag,
            status: self.status.clone(),
            events: events_tx,
        };
        let rescan = self.rescan.clone();
        let mut ports: Vec<Port> = Vec::new();
        let mut next_port_key: u64 = 0;
        let mut fallback = tokio::time::interval(FALLBACK_SCAN_INTERVAL);

        loop {
            // The arrived key crosses the select so the seam is free again before the
            // inbound handoff borrows it; every other arm completes in place.
            let arrived = tokio::select! {
                _ = fallback.tick() => {
                    self.reconcile(&mut ports, &context, &port_notify_tx, &mut next_port_key)
                        .await;
                    None
                }
                () = rescan.notified() => {
                    self.reconcile(&mut ports, &context, &port_notify_tx, &mut next_port_key)
                        .await;
                    None
                }
                Some(event) = events_rx.recv() => {
                    match event {
                        PortEvent::Confirmed { id } => {
                            if let Some(port) = ports.iter_mut().find(|port| port.id == id) {
                                port.confirmed = true;
                            }
                        }
                        PortEvent::Closed { id } => {
                            ports.retain(|port| port.id != id);
                        }
                    }
                    self.refresh_connection(&ports);
                    None
                }
                Some(key) = port_notify_rx.recv() => Some(key),
                out = seam.next_outbound() => {
                    for port in &mut ports {
                        if port.confirmed {
                            if let Some(slot) = port.outbound.try_grant() {
                                slot.fill(out);
                                port.outbound.commit();
                            }
                        }
                    }
                    None
                }
            };
            if let Some(key) = arrived {
                let Some(port) = ports.iter_mut().find(|port| port.key == key) else {
                    continue;
                };
                let Some(slot) = port.inbound.try_peek() else {
                    continue;
                };
                seam.next_inbound(slot.frame()).await;
                port.inbound.release();
            }
        }
    }
}

impl<Scan, Open, Fut, S> UsbAutoHost<Scan, Open>
where
    Scan: FnMut() -> Vec<String> + Send + 'static,
    Open: FnMut(String) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    /// Re-enumerate the present CDC ports: drop (and abort) the tasks of any that vanished, spawn
    /// a fresh task for any newly present, and refresh the connection state.
    async fn reconcile(
        &mut self,
        ports: &mut Vec<Port>,
        context: &PortContext,
        port_notify: &UnboundedSender<u64>,
        next_port_key: &mut u64,
    ) {
        let present = (self.scan)();
        ports.retain(|port| {
            if present.iter().any(|name| name == &port.id) {
                true
            } else {
                port.task.abort();
                false
            }
        });
        for name in present {
            if ports.iter().any(|port| port.id == name) {
                continue;
            }
            if let Ok(stream) = (self.open)(name.clone()).await {
                let key = *next_port_key;
                *next_port_key += 1;
                let (in_tx, in_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(PORT_LANE_DEPTH);
                let (out_tx, out_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(PORT_LANE_DEPTH);
                let task = tokio::spawn(serve_port(
                    name.clone(),
                    stream,
                    context.clone(),
                    in_tx,
                    port_notify.clone(),
                    key,
                    out_rx,
                ));
                ports.push(Port {
                    id: name,
                    key,
                    confirmed: false,
                    outbound: out_tx,
                    inbound: in_rx,
                    task,
                });
            }
        }
        self.refresh_connection(ports);
    }
}

/// The next frame owed to this port's wire, borrowed in place from its lane; the borrow
/// releases on the following call — the seam's own outbound discipline, one level down.
async fn next_from_lane<const SLOT: usize>(lane: &mut TokioGrantConsumer<SLOT>) -> &[u8] {
    lane.release();
    lane.peek().await.frame()
}

/// Serve one CDC port: probe it with `Hello` until it answers, then deframe its inbound data
/// into the port's own grant lane (announcing each commit on the hub's notify funnel) and write
/// any broadcast outbound onto its wire. Returns on any IO error so the run loop prunes it.
#[allow(clippy::too_many_arguments)]
async fn serve_port<S>(
    id: String,
    mut stream: S,
    context: PortContext,
    mut inbound: TokioGrantProducer<MAX_WIRE_FRAME_LEN>,
    notify: UnboundedSender<u64>,
    key: u64,
    mut outbound: TokioGrantConsumer<MAX_WIRE_FRAME_LEN>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut decoder = core::Decoder::new();
    let mut read_buf = [0u8; core::READ_CHUNK_BYTES];
    let mut frame_buf = [0u8; core::MAX_FRAMED_BYTES];
    let mut confirmed = false;
    let mut probe = tokio::time::interval(PROBE_INTERVAL);

    loop {
        tokio::select! {
            _ = probe.tick(), if !confirmed => {
                let hello = Message::Hello(Capabilities::host());
                if write_message(&mut stream, &hello, &mut frame_buf, &context.status)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            read = stream.read(&mut read_buf) => {
                let n = match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                context.status.add_rx(n as u64);
                let mut write_failed = false;
                for &byte in &read_buf[..n] {
                    let Ok(Some(frame)) = decoder.feed(byte) else {
                        continue;
                    };
                    if frame.is_empty() {
                        continue;
                    }
                    match core::host_react(core::decode_message(frame)) {
                        HostInbound::AnswerHandshake => {
                            let ack = Message::HelloAck {
                                tag: context.node_tag,
                                capabilities: Capabilities::host(),
                            };
                            if write_message(&mut stream, &ack, &mut frame_buf, &context.status)
                                .await
                                .is_err()
                            {
                                write_failed = true;
                                break;
                            }
                            confirm(&mut confirmed, &id, &context.events);
                        }
                        HostInbound::Confirmed(_) => confirm(&mut confirmed, &id, &context.events),
                        HostInbound::Data(packet) => {
                            if confirmed && !packet.is_empty() {
                                inbound.grant().await.fill(packet);
                                inbound.commit();
                                let _ = notify.send(key);
                            }
                        }
                        HostInbound::Ignore => {}
                    }
                }
                if write_failed {
                    break;
                }
            }
            out = next_from_lane(&mut outbound) => {
                let data = Message::Data(out);
                if write_message(&mut stream, &data, &mut frame_buf, &context.status)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = context.events.send(PortEvent::Closed { id });
}

fn confirm(confirmed: &mut bool, id: &str, events: &UnboundedSender<PortEvent>) {
    if !*confirmed {
        *confirmed = true;
        let _ = events.send(PortEvent::Confirmed { id: id.to_string() });
    }
}

async fn write_message<S>(
    stream: &mut S,
    message: &Message<'_>,
    frame_buf: &mut [u8; core::MAX_FRAMED_BYTES],
    status: &TokioInterfaceStatus,
) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let Ok(n) = message.write_framed(frame_buf) else {
        return Ok(());
    };
    stream.write_all(&frame_buf[..n]).await?;
    status.add_tx(n as u64);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InterfaceStatus;
    use crate::reactor::impls::tokio_reactor::TokioInterfaceSeam;
    use std::time::Duration;
    use tokio::io::AsyncRead;
    use tokio::sync::mpsc::unbounded_channel;

    fn host_id() -> InterfaceId {
        InterfaceId::new([0xD0; 16])
    }

    /// Read the device end until a decoded frame satisfies `pick`, returning what it picks.
    async fn read_until<R, T>(
        wire: &mut R,
        decoder: &mut core::Decoder,
        mut pick: impl FnMut(Message<'_>) -> Option<T>,
    ) -> T
    where
        R: AsyncRead + Unpin,
    {
        let mut buf = [0u8; 64];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(2), wire.read(&mut buf))
                .await
                .expect("a frame arrives within the window")
                .expect("the device wire stays open");
            for &byte in &buf[..n] {
                let Ok(Some(frame)) = decoder.feed(byte) else {
                    continue;
                };
                if frame.is_empty() {
                    continue;
                }
                if let Ok(message) = core::decode_message(frame) {
                    if let Some(picked) = pick(message) {
                        return picked;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn the_host_handshakes_a_discovered_port_then_carries_data_both_ways() {
        let (host_wire, mut device) = tokio::io::duplex(4096);
        let mut host_wire = Some(host_wire);
        let open = move |_name: String| {
            let taken = host_wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let scan = || std::vec![String::from("loopback")];

        let host = UsbAutoHost::new(host_id(), scan, open, Arc::new(Notify::new()));
        let status = host.status();

        let (notify_tx, mut notify_rx) = unbounded_channel::<InterfaceId>();
        let (in_tx, mut in_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(8);
        let (mut out_tx, out_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(8);
        let seam = TokioInterfaceSeam::new(host_id(), in_tx, notify_tx, out_rx);
        tokio::spawn(host.run(seam));

        // The host probes the newly discovered port with a Hello; the device answers HelloAck and
        // the host confirms the link (its status turns Connected).
        let mut decoder = core::Decoder::new();
        read_until(&mut device, &mut decoder, |message| {
            matches!(message, Message::Hello(_)).then_some(())
        })
        .await;

        let mut frame = [0u8; core::MAX_FRAMED_BYTES];
        let ack = Message::HelloAck {
            tag: NodeTag([0xAB; 8]),
            capabilities: Capabilities::none(),
        };
        let n = ack.write_framed(&mut frame).expect("frames the ack");
        device.write_all(&frame[..n]).await.expect("the host reads");

        tokio::time::timeout(Duration::from_secs(2), async {
            while status.connection() != ConnectionState::Connected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the host confirms the link within the window");

        // Outbound fans out: a frame granted into the egress lane reaches the confirmed port
        // as a Data frame.
        let outbound_packet = [0x11u8, 0x22, 0x33];
        out_tx
            .try_grant()
            .expect("the egress lane has a free slot")
            .fill(&outbound_packet);
        out_tx.commit();
        let delivered = read_until(&mut device, &mut decoder, |message| match message {
            Message::Data(packet) => Some(packet.to_vec()),
            _ => None,
        })
        .await;
        assert_eq!(delivered, outbound_packet);

        // Inbound fans in: a Data frame from the device lands in the host's grant lane,
        // announced on the notify funnel with the host's id.
        let inbound_packet = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let n = Message::Data(&inbound_packet)
            .write_framed(&mut frame)
            .expect("frames the data");
        device.write_all(&frame[..n]).await.expect("the host reads");
        let announced = tokio::time::timeout(Duration::from_secs(2), notify_rx.recv())
            .await
            .expect("the inbound frame funnels within the window")
            .expect("the host task is alive");
        assert_eq!(announced, host_id());
        let received = in_rx
            .try_peek()
            .expect("the announced frame is in the lane");
        assert_eq!(received.frame(), &inbound_packet);
        in_rx.release();
    }
}
