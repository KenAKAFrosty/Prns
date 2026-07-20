//! The tokio host side of the plug-and-play USB-auto interface: a hub that discovers CDC
//! ports, handshakes each, and multiplexes them all behind one [`InterfaceId`]. Each port is
//! its own async task that sleeps on its wire and funnels straight into the reactor's inbound,
//! and discovery is event-driven (the consumer pokes a rescan signal on OS hot-plug), so a
//! board appears the moment it is plugged.
//!
//! Inbound fans IN: every confirmed port fills its own grant lane and announces the commit on
//! the hub's port-notify funnel (the reactor's own id-funnel pattern, one level down).
//! Outbound fans OUT: the run loop write-grants each borrowed frame into every confirmed port's lane.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::vec::Vec;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use prns_core::interfaces::usb_auto::core::{self, Capabilities, HostInbound, Message, NodeTag};
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor,
    InterfaceId, InterfaceKind,
};
use prns_runtime::reactor::driver::{
    tokio_grant_lane, TokioGrantConsumer, TokioGrantProducer, TokioInterfaceStatus,
};
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};

/// A slow fallback re-enumeration. Hot-plug is event-driven (the consumer pokes the rescan
/// signal the instant the OS reports a change), so this only backstops a missed event, a host
/// with no hot-plug source (e.g. macOS), and re-opening a port whose task died without an unplug.
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(1);
/// How often a not-yet-confirmed port re-sends its `Hello` — covering a board that was still
/// booting when first opened, with no replug needed.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// Briefly keep a just-confirmed host link in `Degraded` rather than dropping straight to
/// `Disconnected`. Some Android USB host stacks close and reopen CDC pipes during otherwise
/// healthy traffic; this keeps the app's liveness from flickering Dormant between re-handshakes.
const RECENT_LINK_GRACE: Duration = Duration::from_secs(3);
/// A failed open usually means another process still owns the serial/USB interface or the device is
/// mid-reenumeration. Back off per target so a busy interface does not turn into a once-per-second
/// error storm.
const OPEN_FAILURE_BACKOFF: Duration = Duration::from_secs(5);

struct Port {
    id: String,
    key: u64,
    confirmed: bool,
    outbound: TokioGrantProducer,
    inbound: TokioGrantConsumer,
    task: JoinHandle<()>,
}

enum PortEvent {
    Confirmed { id: String },
    Closed { id: String },
}

type PendingOpen<S> = Pin<Box<dyn Future<Output = (String, io::Result<S>)> + Send>>;

fn has_pending_retry(failed_opens: &HashMap<String, Instant>) -> bool {
    failed_opens
        .values()
        .any(|failed_at| failed_at.elapsed() < OPEN_FAILURE_BACKOFF)
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
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    rescan: Arc<Notify>,
}

impl<Scan, Open> UsbAutoHost<Scan, Open> {
    #[must_use]
    pub fn new(id: InterfaceId, scan: Scan, open: Open, rescan: Arc<Notify>) -> Self {
        Self::with_policy(
            id,
            scan,
            open,
            rescan,
            core::HOST_DEFAULTS.configured(ConfiguredInterfacePolicy::default()),
        )
    }

    #[must_use]
    pub fn with_policy(
        id: InterfaceId,
        scan: Scan,
        open: Open,
        rescan: Arc<Notify>,
        policy: EffectiveInterfacePolicy,
    ) -> Self {
        Self {
            id,
            node_tag: core::node_tag_for(id),
            scan,
            open,
            policy,
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

    fn refresh_connection(
        &self,
        ports: &[Port],
        has_pending_open: bool,
        last_confirmed_at: Option<Instant>,
    ) {
        let connection = if ports.iter().any(|port| port.confirmed) {
            ConnectionState::Connected
        } else if last_confirmed_at
            .map(|confirmed_at| confirmed_at.elapsed() < RECENT_LINK_GRACE)
            .unwrap_or(false)
        {
            ConnectionState::Degraded
        } else if has_pending_open || !ports.is_empty() {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Disconnected
        };
        self.status.set_connection(connection);
    }
}

impl<Scan, Open, Fut, S> Interface for UsbAutoHost<Scan, Open>
where
    Scan: FnMut() -> Vec<String> + Send + 'static,
    Open: FnMut(String) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    const HW_MTU: usize = prns_core::interfaces::usb_auto::core::HOST_USB_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::UsbAutoHost;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.policy.descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
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
        let mut opening: HashSet<String> = HashSet::new();
        let mut failed_opens: HashMap<String, Instant> = HashMap::new();
        let mut pending_opens: FuturesUnordered<PendingOpen<S>> = FuturesUnordered::new();
        let mut next_port_key: u64 = 0;
        let mut last_confirmed_at: Option<Instant> = None;
        let mut fallback = tokio::time::interval(FALLBACK_SCAN_INTERVAL);

        loop {
            // The arrived key crosses the select so the seam is free again before the
            // inbound handoff borrows it; every other arm completes in place.
            let arrived = tokio::select! {
                _ = fallback.tick() => {
                    self.reconcile(
                        &mut ports,
                        &mut opening,
                        &mut failed_opens,
                        &mut pending_opens,
                        last_confirmed_at,
                    )
                        .await;
                    None
                }
                () = rescan.notified() => {
                    self.reconcile(
                        &mut ports,
                        &mut opening,
                        &mut failed_opens,
                        &mut pending_opens,
                        last_confirmed_at,
                    )
                        .await;
                    None
                }
                () = self.status.wait_until_disabled(), if self.status.is_enabled() => {
                    self.reconcile(
                        &mut ports,
                        &mut opening,
                        &mut failed_opens,
                        &mut pending_opens,
                        last_confirmed_at,
                    )
                        .await;
                    None
                }
                () = self.status.wait_until_enabled(), if !self.status.is_enabled() => {
                    self.reconcile(
                        &mut ports,
                        &mut opening,
                        &mut failed_opens,
                        &mut pending_opens,
                        last_confirmed_at,
                    )
                        .await;
                    None
                }
                Some(event) = events_rx.recv() => {
                    match event {
                        PortEvent::Confirmed { id } => {
                            if let Some(port) = ports.iter_mut().find(|port| port.id == id) {
                                port.confirmed = true;
                                last_confirmed_at = Some(Instant::now());
                            }
                        }
                        PortEvent::Closed { id } => {
                            ports.retain(|port| port.id != id);
                        }
                    }
                    self.refresh_connection(
                        &ports,
                        !opening.is_empty() || has_pending_retry(&failed_opens),
                        last_confirmed_at,
                    );
                    None
                }
                Some((name, opened)) = pending_opens.next(), if !pending_opens.is_empty() => {
                    opening.remove(&name);
                    match opened {
                        Ok(stream) => {
                            let key = next_port_key;
                            next_port_key += 1;
                            let (in_tx, in_rx) =
                                tokio_grant_lane(core::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
                            let (out_tx, out_rx) =
                                tokio_grant_lane(core::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
                            let task = tokio::spawn(serve_port(
                                name.clone(),
                                stream,
                                context.clone(),
                                in_tx,
                                port_notify_tx.clone(),
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
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            crate::diagnostic_log::debug!(
                                "usb-auto: {name} requested re-enumeration"
                            );
                        }
                        Err(error) => {
                            crate::diagnostic_log::warn!("usb-auto: open {name} failed: {error}");
                            failed_opens.insert(name, Instant::now());
                        }
                    }
                    self.refresh_connection(
                        &ports,
                        !opening.is_empty() || has_pending_retry(&failed_opens),
                        last_confirmed_at,
                    );
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
    Fut: Future<Output = io::Result<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    /// Re-enumerate the present CDC ports: drop (and abort) the tasks of any that vanished, spawn
    /// a fresh task for any newly present, and refresh the connection state.
    async fn reconcile(
        &mut self,
        ports: &mut Vec<Port>,
        opening: &mut HashSet<String>,
        failed_opens: &mut HashMap<String, Instant>,
        pending_opens: &mut FuturesUnordered<PendingOpen<S>>,
        last_confirmed_at: Option<Instant>,
    ) {
        if !self.status.is_enabled() {
            // Off: drop every port (aborting its task releases the held serial FD) and report
            // Disabled, so the OS frees the device for other readers — a log monitor — until resume.
            for port in ports.drain(..) {
                port.task.abort();
            }
            opening.clear();
            failed_opens.clear();
            *pending_opens = FuturesUnordered::new();
            self.status.set_connection(ConnectionState::Disabled);
            return;
        }
        let present = (self.scan)();
        ports.retain(|port| {
            if present.iter().any(|name| name == &port.id) {
                true
            } else {
                port.task.abort();
                false
            }
        });
        opening.retain(|name| present.iter().any(|present_name| present_name == name));
        failed_opens.retain(|name, _| present.iter().any(|present_name| present_name == name));
        for name in present {
            if ports.iter().any(|port| port.id == name) || opening.contains(&name) {
                continue;
            }
            if failed_opens
                .get(&name)
                .is_some_and(|failed_at| failed_at.elapsed() < OPEN_FAILURE_BACKOFF)
            {
                continue;
            }
            failed_opens.remove(&name);
            let future = (self.open)(name.clone());
            opening.insert(name.clone());
            pending_opens.push(Box::pin(async move {
                let opened = future.await;
                (name, opened)
            }));
        }
        self.refresh_connection(
            ports,
            !opening.is_empty() || has_pending_retry(failed_opens),
            last_confirmed_at,
        );
    }
}

/// The next frame owed to this port's wire, borrowed in place from its lane; the borrow
/// releases on the following call — the seam's own outbound discipline, one level down.
async fn next_from_lane(lane: &mut TokioGrantConsumer) -> &[u8] {
    lane.release();
    lane.peek().await.frame()
}

/// Serve one CDC port: probe it with `Hello` until it answers, then deframe its inbound data
/// into the port's own grant lane (announcing each commit on the hub's notify funnel) and write
/// any broadcast outbound onto its wire. Returns on any IO error so the run loop prunes it.
async fn serve_port<S>(
    id: String,
    mut stream: S,
    context: PortContext,
    mut inbound: TokioGrantProducer,
    notify: UnboundedSender<u64>,
    key: u64,
    mut outbound: TokioGrantConsumer,
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
    stream.flush().await?;
    status.add_tx(n as u64);
    Ok(())
}

impl<Scan, Open> prns_core::interfaces::ReportsStatus for UsbAutoHost<Scan, Open> {
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
    use prns_core::interfaces::InterfaceStatus;
    use prns_runtime::reactor::driver::TokioInterfaceSeam;
    use std::time::Duration;
    use tokio::io::AsyncRead;
    use tokio::sync::mpsc::unbounded_channel;

    fn host_id() -> InterfaceId {
        InterfaceId::new([0xD0; 8])
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
        let (in_tx, mut in_rx) = tokio_grant_lane(core::MAX_FRAMED_BYTES, 8);
        let (mut out_tx, out_rx) = tokio_grant_lane(core::MAX_FRAMED_BYTES, 8);
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

    #[test]
    fn recently_confirmed_link_lingers_degraded_instead_of_disconnected() {
        let open = |_name: String| async {
            Err::<tokio::io::DuplexStream, io::Error>(io::ErrorKind::NotConnected.into())
        };
        let host = UsbAutoHost::new(host_id(), Vec::<String>::new, open, Arc::new(Notify::new()));
        let status = host.status();

        host.refresh_connection(&[], false, Some(Instant::now()));
        assert_eq!(status.connection(), ConnectionState::Degraded);

        host.refresh_connection(
            &[],
            false,
            Some(Instant::now() - RECENT_LINK_GRACE - Duration::from_millis(1)),
        );
        assert_eq!(status.connection(), ConnectionState::Disconnected);

        host.refresh_connection(&[], true, None);
        assert_eq!(status.connection(), ConnectionState::Reconnecting);
    }

    #[test]
    fn configured_policy_reaches_the_usb_auto_descriptor() {
        let policy = core::HOST_DEFAULTS.configured(ConfiguredInterfacePolicy {
            mode: Some(prns_core::interfaces::InterfaceMode::Gateway),
            bitrate: Some(prns_core::interfaces::BitrateBps::guess(7_654_321)),
            ..ConfiguredInterfacePolicy::default()
        });
        let open = |_name: String| async {
            Err::<tokio::io::DuplexStream, io::Error>(io::ErrorKind::NotConnected.into())
        };
        let host = UsbAutoHost::with_policy(
            host_id(),
            Vec::<String>::new,
            open,
            Arc::new(Notify::new()),
            policy,
        );

        assert_eq!(host.descriptor(), policy.descriptor(host_id()));
    }
}
