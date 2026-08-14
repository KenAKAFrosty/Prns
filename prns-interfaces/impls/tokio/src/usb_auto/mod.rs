mod host;

pub use host::{AutoUsb, DEFAULT_USB_AUTO_ID, DEFAULT_USB_BAUD};

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

use prns_core::interfaces::usb_auto::{
    self as contract, Capabilities, HostInbound, Message, NodeTag,
};
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor,
    InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::driver::{
    tokio_grant_lane, TokioGrantConsumer, TokioGrantProducer, TokioInterfaceStatus,
};
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};

/// Backstops missed hot-plug events, platforms without a hot-plug source, and ports whose task died without an unplug.
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(1);
/// Repeats the handshake while a newly opened board may still be booting.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(6);
/// Masks brief CDC close-and-reopen cycles on Android without reporting a disconnected link between handshakes.
const RECENT_LINK_GRACE: Duration = Duration::from_secs(3);
/// Backs off a busy or re-enumerating target so failures do not become a once-per-second error storm.
const OPEN_FAILURE_BACKOFF: Duration = Duration::from_secs(5);
/// Gives up a port that accepts writes and never answers, so a serial device that is not a board
/// stops being held open. Long enough to cover a board that resets and boots after the port opens.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
/// Stops reopening a port that has never answered, until the entry re-enumerates. Without this the
/// deadline alone still holds the device for `HANDSHAKE_DEADLINE` out of every backoff cycle.
const MAX_UNCONFIRMED_OPENS: u32 = 3;

struct Port {
    id: String,
    key: u64,
    liveness: PortLiveness,
    outbound: TokioGrantProducer,
    inbound: TokioGrantConsumer,
    task: JoinHandle<()>,
}

#[derive(Default)]
enum PortLiveness {
    #[default]
    Handshaking,
    Alive(Instant),
}

impl PortLiveness {
    fn mark_alive(&mut self, now: Instant) {
        *self = Self::Alive(now);
    }

    fn is_alive(&self, now: Instant) -> bool {
        match self {
            Self::Handshaking => false,
            Self::Alive(last_seen) => now.duration_since(*last_seen) < LIVENESS_TIMEOUT,
        }
    }
}

enum PortEvent {
    Alive {
        id: String,
    },
    /// `confirmed` distinguishes a link that ended from one that never started, which is what
    /// decides whether the port is worth reopening.
    Closed {
        id: String,
        confirmed: bool,
    },
}

type PendingOpen<S> = Pin<Box<dyn Future<Output = (String, io::Result<S>)> + Send>>;

/// Why a port is not being opened right now, and how many times it has already been given up on.
struct OpenFailure {
    at: Instant,
    unconfirmed: u32,
}

impl OpenFailure {
    fn since(at: Instant) -> Self {
        Self { at, unconfirmed: 0 }
    }

    fn is_abandoned(&self) -> bool {
        self.unconfirmed >= MAX_UNCONFIRMED_OPENS
    }

    fn is_backing_off(&self) -> bool {
        !self.is_abandoned() && self.at.elapsed() < OPEN_FAILURE_BACKOFF
    }
}

fn has_pending_retry(failed_opens: &HashMap<String, OpenFailure>) -> bool {
    failed_opens.values().any(OpenFailure::is_backing_off)
}

/// Records a port worth backing off from, keeping the unconfirmed tally across reopens so the
/// give-up threshold counts attempts rather than resetting on every retry.
fn note_failure(failed_opens: &mut HashMap<String, OpenFailure>, id: String, unconfirmed: bool) {
    let now = Instant::now();
    let failure = failed_opens
        .entry(id)
        .or_insert_with(|| OpenFailure::since(now));
    failure.at = now;
    if unconfirmed {
        failure.unconfirmed += 1;
    }
}

#[derive(Clone)]
struct PortContext {
    node_tag: NodeTag,
    status: TokioInterfaceStatus,
    events: UnboundedSender<PortEvent>,
}

/// Port reads backpressure when inbound is full; broadcast fan-out drops for a full outbound lane.
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
            contract::HOST_DEFAULTS.configured(ConfiguredInterfacePolicy::default()),
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
            node_tag: contract::node_tag_for(id),
            scan,
            open,
            policy,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
            rescan,
        }
    }

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
        let now = Instant::now();
        let connection = if ports.iter().any(|port| port.liveness.is_alive(now)) {
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
    const HW_MTU: usize = prns_core::interfaces::usb_auto::HOST_USB_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::UsbAutoHost;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.policy.descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

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
        let mut failed_opens: HashMap<String, OpenFailure> = HashMap::new();
        let mut pending_opens: FuturesUnordered<PendingOpen<S>> = FuturesUnordered::new();
        let mut next_port_key: u64 = 0;
        let mut last_confirmed_at: Option<Instant> = None;
        let mut fallback = tokio::time::interval(FALLBACK_SCAN_INTERVAL);

        loop {
            // The arrived key crosses the select so the seam is released before the inbound handoff borrows it.
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
                        PortEvent::Alive { id } => {
                            if let Some(port) = ports.iter_mut().find(|port| port.id == id) {
                                let now = Instant::now();
                                port.liveness.mark_alive(now);
                                last_confirmed_at = Some(now);
                            }
                            // A port that answered has earned its retries back.
                            failed_opens.remove(&id);
                        }
                        PortEvent::Closed { id, confirmed } => {
                            ports.retain(|port| port.id != id);
                            if !confirmed {
                                note_failure(&mut failed_opens, id.clone(), true);
                                if failed_opens.get(&id).is_some_and(OpenFailure::is_abandoned) {
                                    crate::diagnostic_log::warn!(
                                        "usb-auto: {id} never answered a handshake in {MAX_UNCONFIRMED_OPENS} opens; leaving it alone until it re-enumerates"
                                    );
                                }
                            }
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
                                tokio_grant_lane(contract::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
                            let (out_tx, out_rx) =
                                tokio_grant_lane(contract::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
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
                                liveness: PortLiveness::default(),
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
                            note_failure(&mut failed_opens, name, false);
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
                    let now = Instant::now();
                    for port in &mut ports {
                        if port.liveness.is_alive(now) {
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
    async fn reconcile(
        &mut self,
        ports: &mut Vec<Port>,
        opening: &mut HashSet<String>,
        failed_opens: &mut HashMap<String, OpenFailure>,
        pending_opens: &mut FuturesUnordered<PendingOpen<S>>,
        last_confirmed_at: Option<Instant>,
    ) {
        if !self.status.is_enabled() {
            // Dropping every port releases its serial descriptor for other readers while the interface is disabled.
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
            // The tally survives the retry: clearing it here would reset the give-up threshold on
            // every reopen. `retain` above drops it when the entry leaves the scan.
            if failed_opens
                .get(&name)
                .is_some_and(|failure| failure.is_abandoned() || failure.is_backing_off())
            {
                continue;
            }
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

/// Releases the previous lane borrow before returning the next frame.
async fn next_from_lane(lane: &mut TokioGrantConsumer) -> &[u8] {
    lane.release();
    lane.peek().await.frame()
}

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
    let mut decoder = contract::Decoder::new();
    let mut read_buf = [0u8; contract::READ_CHUNK_BYTES];
    let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];
    let mut confirmed = false;
    let mut probe = tokio::time::interval(PROBE_INTERVAL);
    let mut liveness_probe = tokio::time::interval(LIVENESS_PROBE_INTERVAL);
    let handshake_deadline = Instant::now() + HANDSHAKE_DEADLINE;

    loop {
        tokio::select! {
            () = tokio::time::sleep_until(handshake_deadline), if !confirmed => {
                crate::diagnostic_log::warn!(
                    "usb-auto: {id} did not answer a handshake in {HANDSHAKE_DEADLINE:?}; releasing the port"
                );
                break;
            }
            _ = probe.tick(), if !confirmed => {
                let hello = Message::Hello(Capabilities::host());
                if write_message(&mut stream, &hello, &mut frame_buf, &context.status)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            _ = liveness_probe.tick(), if confirmed => {
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
                    match contract::host_react(contract::decode_message(frame)) {
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
                            mark_alive(&mut confirmed, &id, &context.events);
                        }
                        HostInbound::Confirmed(_) => {
                            mark_alive(&mut confirmed, &id, &context.events)
                        }
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
    let _ = context.events.send(PortEvent::Closed { id, confirmed });
}

fn mark_alive(confirmed: &mut bool, id: &str, events: &UnboundedSender<PortEvent>) {
    *confirmed = true;
    let _ = events.send(PortEvent::Alive { id: id.to_string() });
}

async fn write_message<S>(
    stream: &mut S,
    message: &Message<'_>,
    frame_buf: &mut [u8; contract::MAX_FRAMED_BYTES],
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
    use prns_runtime::manifold::driver::TokioInterfaceSeam;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::AsyncRead;
    use tokio::sync::mpsc::unbounded_channel;

    const FRAME_ARRIVAL_TIMEOUT: Duration =
        LIVENESS_PROBE_INTERVAL.saturating_add(Duration::from_secs(1));

    fn host_id() -> InterfaceId {
        InterfaceId::new([0xD0; 8])
    }

    async fn read_until<R, T>(
        wire: &mut R,
        decoder: &mut contract::Decoder,
        mut pick: impl FnMut(Message<'_>) -> Option<T>,
    ) -> T
    where
        R: AsyncRead + Unpin,
    {
        let mut buf = [0u8; 64];
        loop {
            let n = tokio::time::timeout(FRAME_ARRIVAL_TIMEOUT, wire.read(&mut buf))
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
                if let Ok(message) = contract::decode_message(frame) {
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
        let (in_tx, mut in_rx) = tokio_grant_lane(contract::MAX_FRAMED_BYTES, 8);
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::MAX_FRAMED_BYTES, 8);
        let seam = TokioInterfaceSeam::new(host_id(), in_tx, notify_tx, out_rx);
        tokio::spawn(host.run(seam));

        let mut decoder = contract::Decoder::new();
        read_until(&mut device, &mut decoder, |message| {
            matches!(message, Message::Hello(_)).then_some(())
        })
        .await;

        let mut frame = [0u8; contract::MAX_FRAMED_BYTES];
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

        read_until(&mut device, &mut decoder, |message| {
            matches!(message, Message::Hello(_)).then_some(())
        })
        .await;
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

    #[tokio::test(start_paused = true)]
    async fn an_enumerated_port_becomes_reconnecting_when_liveness_expires() {
        let open = |_name: String| async {
            Err::<tokio::io::DuplexStream, io::Error>(io::ErrorKind::NotConnected.into())
        };
        let host = UsbAutoHost::new(host_id(), Vec::<String>::new, open, Arc::new(Notify::new()));
        let status = host.status();
        let (outbound, _outbound_rx) =
            tokio_grant_lane(contract::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
        let (_inbound_tx, inbound) = tokio_grant_lane(contract::MAX_FRAMED_BYTES, PORT_LANE_DEPTH);
        let ports = [Port {
            id: String::from("loopback"),
            key: 0,
            liveness: PortLiveness::Alive(Instant::now()),
            outbound,
            inbound,
            task: tokio::spawn(async {}),
        }];

        host.refresh_connection(&ports, false, None);
        assert_eq!(status.connection(), ConnectionState::Connected);

        tokio::time::advance(LIVENESS_TIMEOUT).await;
        host.refresh_connection(&ports, false, None);
        assert_eq!(status.connection(), ConnectionState::Reconnecting);
    }

    #[test]
    fn configured_policy_reaches_the_usb_auto_descriptor() {
        let policy = contract::HOST_DEFAULTS.configured(ConfiguredInterfacePolicy {
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

    /// Any serial device that is not a board: it accepts the handshake writes and never answers.
    /// Draining keeps the wire open, so the host sees a live port rather than an EOF.
    fn silent_wire() -> tokio::io::DuplexStream {
        let (host_wire, mut device) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut sink = [0u8; 256];
            while device.read(&mut sink).await.is_ok_and(|n| n > 0) {}
        });
        host_wire
    }

    /// The seam's peers, held by the caller so they stay open for the life of the test.
    type HeldSeam = (
        mpsc::UnboundedReceiver<InterfaceId>,
        TokioGrantConsumer,
        TokioGrantProducer,
    );

    fn spawn_host<Scan, Open, Fut, S>(scan: Scan, open: Open) -> HeldSeam
    where
        Scan: FnMut() -> Vec<String> + Send + 'static,
        Open: FnMut(String) -> Fut + Send + 'static,
        Fut: Future<Output = io::Result<S>> + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let host = UsbAutoHost::new(host_id(), scan, open, Arc::new(Notify::new()));
        let (notify_tx, notify_rx) = unbounded_channel::<InterfaceId>();
        let (in_tx, in_rx) = tokio_grant_lane(contract::MAX_FRAMED_BYTES, 8);
        let (out_tx, out_rx) = tokio_grant_lane(contract::MAX_FRAMED_BYTES, 8);
        let seam = TokioInterfaceSeam::new(host_id(), in_tx, notify_tx, out_rx);
        tokio::spawn(host.run(seam));
        (notify_rx, in_rx, out_tx)
    }

    #[tokio::test(start_paused = true)]
    async fn a_port_that_never_answers_the_handshake_is_released() {
        let (host_wire, mut device) = tokio::io::duplex(4096);
        let mut host_wire = Some(host_wire);
        let open = move |_name: String| {
            let taken = host_wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let _seam = spawn_host(|| std::vec![String::from("silent")], open);

        // Before the deadline existed this never returned: the read arm only ends on EOF, and
        // `PortLiveness::Handshaking` never ages out, so the descriptor was held until unplug.
        let mut sink = [0u8; 256];
        tokio::time::timeout(HANDSHAKE_DEADLINE * 4, async {
            while device.read(&mut sink).await.is_ok_and(|n| n > 0) {}
        })
        .await
        .expect("the host drops a port that has not answered by the handshake deadline");
    }

    #[tokio::test(start_paused = true)]
    async fn a_port_that_never_answers_stops_being_reopened() {
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = opens.clone();
        let open = move |_name: String| {
            counted.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, io::Error>(silent_wire()) }
        };
        let _seam = spawn_host(|| std::vec![String::from("silent")], open);

        let cycle = HANDSHAKE_DEADLINE + OPEN_FAILURE_BACKOFF + FALLBACK_SCAN_INTERVAL;
        tokio::time::sleep(cycle * (MAX_UNCONFIRMED_OPENS + 3)).await;

        assert_eq!(
            opens.load(Ordering::SeqCst),
            MAX_UNCONFIRMED_OPENS as usize,
            "a port that never answers is reopened until the give-up threshold and then left alone"
        );
    }
}
