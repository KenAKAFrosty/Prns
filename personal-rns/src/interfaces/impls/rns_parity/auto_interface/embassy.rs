//! The embassy-net worker shell for the RNS AutoInterface — the ESP32 family
//! (S3, C6) and any embassy-net host.
//!
//!  The host supplies the WiFi/IP stack, the
//! channels (`'static`), the interface id, and an entropy source; everything
//! about *how this talks to the world* stays in here.

use core::net::Ipv6Addr;
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use embassy_futures::select::{select, select4, Either, Either4};
use embassy_net::udp::{PacketMetadata, RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant as EmbassyInstant, Ticker};
use heapless::Vec as HVec;

use super::core::{
    AutoInterfaceProtocol, DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT, DISCOVERY_GROUP,
    HARDWARE_MTU, UNICAST_DISCOVERY_PORT,
};
use crate::engine::{InstantMillis, OutboundPacket};
use crate::interfaces::{
    Capabilities, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceMode, InterfaceStats,
    InterfaceWorker, LinkState, MacAddress, MediumKind, QueueFull, TrackedPeerMulticastInterface,
};
use crate::runtime::manifold::impls::embassy::{InboundSender, InboxEntry};

pub const OUTBOX_DEPTH: usize = 4;
const CHANNEL_PACKET_LEN_CAP: usize = HARDWARE_MTU;
pub type PacketBuf = HVec<u8, CHANNEL_PACKET_LEN_CAP>;
const MAX_PEERS: usize = 8;
const BEACON_INTERVAL_MS: u64 = 1600;

/// Outbound: packets leaving the system through this worker. Names are from the
/// engine's perspective — *outbound* = headed for the wire — not the channel's.
/// The runtime fills it (the handle holds the [`OutboundSender`], pushed via
/// [`EmbassyAutoInterface::submit`]); the worker drains the [`OutboundReceiver`]
/// and fans each packet out to its peers. It lives here, with the worker — its
/// *draining* end — the same rule that puts the inbound mailbox with the runtime
/// that drains it (`runtime::manifold::impls::embassy`). Sized to this worker's
/// [`HARDWARE_MTU`].
pub type OutboundChannel = Channel<CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundSender = Sender<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundReceiver = Receiver<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;

/// A `'static` liveness flag a host shares between the worker handle and its
/// shell: the shell stores the live link state into it (from `is_link_up`), and
/// the handle's [`health`](EmbassyAutoInterface::health) reads it. `AtomicBool`
/// because it's a single flag touched from two tasks with no ordering needs —
/// a stale read for one render is harmless. The traffic/destination counters an
/// app cares about are metered by the runtime ([`Runtime::snapshot`]), not here.
///
/// [`Runtime::snapshot`]: crate::runtime::Runtime::snapshot
pub type LinkUp = AtomicBool;

pub struct EmbassyAutoInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
    link_up: &'static LinkUp,
    peers: &'static AtomicU16,
}

impl EmbassyAutoInterface {
    /// Build the handle for interface `id`, sending to the worker over
    /// `outbound`, reading liveness from `link_up` and the live group size from
    /// `peers` (the shell writes both).
    pub fn new(
        id: InterfaceId,
        outbound: OutboundSender,
        link_up: &'static LinkUp,
        peers: &'static AtomicU16,
    ) -> Self {
        Self {
            descriptor: InterfaceDescriptor {
                id,
                capabilities: Capabilities {
                    receives: true,
                    transmits: true,
                    forwards: true,
                    // Switched fabric: forwarding is per-peer fanout the engine
                    // drives via fire_on, not source-interface re-propagation.
                    repeats: false,
                },
                mode: InterfaceMode::Full,
                medium: MediumKind::Multicast,
                state: ConnectionState::Connected,
            },
            outbound,
            link_up,
            peers,
        }
    }
}

impl InterfaceWorker for EmbassyAutoInterface {
    // RNS pins the AutoInterface MTU at 1196 bytes; the inbound mailbox sizes to
    // it so a stamped packet always fits.
    const PACKET_BUFFER_SIZE: usize = HARDWARE_MTU;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn health(&self) -> InterfaceStats {
        // Liveness is the live link state the shell publishes into `link_up`.
        // The packet/byte counters belong to the runtime's view (metered in
        // `Runtime::snapshot`), so they stay at their defaults here.
        InterfaceStats {
            link: LinkState::from_up(self.link_up.load(Ordering::Relaxed)),
            ..InterfaceStats::default()
        }
    }

    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull> {
        let buf = PacketBuf::from_slice(packet.bytes).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
    }
}

impl TrackedPeerMulticastInterface for EmbassyAutoInterface {
    fn active_peer_count(&self) -> u16 {
        self.peers.load(Ordering::Relaxed)
    }
}

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

fn ipv6_src(meta: &UdpMetadata) -> Option<Ipv6Addr> {
    match meta.endpoint.addr {
        IpAddress::Ipv6(addr) => Some(addr),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// Handle one discovery-beacon recv: count it, peer on the source, and log a
/// newly discovered peer. The multicast and unicast arms differ only in their
/// socket, buffer, counter, and `via` label, so they share this.
fn ingest_discovery_arm(
    brain: &mut AutoInterfaceProtocol<MAX_PEERS>,
    rx_count: &mut u32,
    recv: Result<(usize, UdpMetadata), RecvError>,
    read_buf: &[u8],
    via: &str,
) {
    match recv {
        Ok((n, meta)) => {
            *rx_count = rx_count.wrapping_add(1);
            if let Some(src) = ipv6_src(&meta) {
                let before = brain.peer_count();
                brain.ingest_discovery_datagram(src, &read_buf[..n], now_millis().0);
                if brain.peer_count() > before {
                    log::info!(
                        "RNS_AUTO PEER+ {src} via {via} (peers={})",
                        brain.peer_count()
                    );
                }
            }
        }
        Err(e) => log::warn!("RNS_AUTO {via} discovery recv err: {e:?}"),
    }
}

/// Handle one inbound data-packet recv: stamp it and hand it to the runtime
/// mailbox, logging a drop if the mailbox is full.
fn ingest_data_arm(
    id: InterfaceId,
    rx_count: &mut u32,
    recv: Result<(usize, UdpMetadata), RecvError>,
    read_buf: &[u8],
    inbound: &InboundSender<HARDWARE_MTU>,
) {
    match recv {
        Ok((n, _meta)) => {
            *rx_count = rx_count.wrapping_add(1);
            if let Ok(bytes) = PacketBuf::from_slice(&read_buf[..n]) {
                let msg = InboxEntry {
                    arrived_at: now_millis(),
                    source: id,
                    bytes,
                };
                if inbound.try_send(msg).is_err() {
                    log::warn!("RNS_AUTO inbound mailbox full, dropped {n}B");
                }
            }
        }
        Err(e) => log::warn!("RNS_AUTO data recv err: {e:?}"),
    }
}

/// The worker's own task/mini-program: own the three UDP sockets, drive the discovery brain,
/// stamp inbound data into `inbound`, and fan `outbound` writes to every peer.
/// Runs forever. `stack` must already be up (host brought up WiFi + IP).
pub async fn run(
    stack: Stack<'static>,
    id: InterfaceId,
    our_mac_address: MacAddress,
    inbound: InboundSender<HARDWARE_MTU>,
    outbound: OutboundReceiver,
    link_up: &'static LinkUp,
    peers: &'static AtomicU16,
) {
    let mut brain = AutoInterfaceProtocol::<MAX_PEERS>::new(our_mac_address);
    log::info!(
        "RNS_AUTO up: link-local {} token {:02x}{:02x}{:02x}{:02x}..",
        brain.our_link_local(),
        brain.our_peering_token().as_bytes()[0],
        brain.our_peering_token().as_bytes()[1],
        brain.our_peering_token().as_bytes()[2],
        brain.our_peering_token().as_bytes()[3],
    );

    match stack.join_multicast_group(IpAddress::Ipv6(DISCOVERY_GROUP)) {
        Ok(()) => log::info!("RNS_AUTO joined {DISCOVERY_GROUP}"),
        Err(e) => log::warn!("RNS_AUTO mcast join failed: {e:?}"),
    }

    let mut discovery_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut discovery_rx_payload = [0u8; 512];
    let mut discovery_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut discovery_tx_payload = [0u8; 512];
    let mut discovery_socket = UdpSocket::new(
        stack,
        &mut discovery_rx_meta,
        &mut discovery_rx_payload,
        &mut discovery_tx_meta,
        &mut discovery_tx_payload,
    );
    discovery_socket
        .bind(DEFAULT_DISCOVERY_PORT)
        .expect("bind discovery port");

    let mut unicast_discovery_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut unicast_discovery_rx_payload = [0u8; 512];
    let mut unicast_discovery_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut unicast_discovery_tx_payload = [0u8; 512];
    let mut unicast_discovery_socket = UdpSocket::new(
        stack,
        &mut unicast_discovery_rx_meta,
        &mut unicast_discovery_rx_payload,
        &mut unicast_discovery_tx_meta,
        &mut unicast_discovery_tx_payload,
    );
    unicast_discovery_socket
        .bind(UNICAST_DISCOVERY_PORT)
        .expect("bind unicast discovery port");

    let mut data_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_rx_payload = [0u8; 1280];
    let mut data_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_tx_payload = [0u8; 1280];
    let mut data_socket = UdpSocket::new(
        stack,
        &mut data_rx_meta,
        &mut data_rx_payload,
        &mut data_tx_meta,
        &mut data_tx_payload,
    );
    data_socket.bind(DEFAULT_DATA_PORT).expect("bind data port");

    let mut discovery_read_buf = [0u8; 64];
    let mut unicast_discovery_read_buf = [0u8; 64];
    let mut data_read_buf = [0u8; HARDWARE_MTU];
    let mut beacon = Ticker::every(Duration::from_millis(BEACON_INTERVAL_MS));
    let mut cycle: u32 = 0;

    // Per-socket RX tallies, logged each tick to catch a socket that goes deaf
    // (a count that stops climbing) after discovering once.
    let mut discovery_rx_count: u32 = 0;
    let mut unicast_discovery_rx_count: u32 = 0;
    let mut data_rx_count: u32 = 0;

    // The shell owns the live link state; the handle's `health()` reads it. The
    // stack is up by the time the host spawns us, so seed it true and refresh
    // each beacon in case WiFi drops.
    link_up.store(true, Ordering::Relaxed);

    loop {
        match select(
            select4(
                discovery_socket.recv_from(&mut discovery_read_buf),
                unicast_discovery_socket.recv_from(&mut unicast_discovery_read_buf),
                data_socket.recv_from(&mut data_read_buf),
                beacon.next(),
            ),
            outbound.receive(),
        )
        .await
        {
            // Multicast discovery beacon (29716).
            Either::First(Either4::First(recv)) => {
                ingest_discovery_arm(
                    &mut brain,
                    &mut discovery_rx_count,
                    recv,
                    &discovery_read_buf,
                    "mcast",
                );
            }

            // Unicast reverse-peering beacon (29717).
            Either::First(Either4::Second(recv)) => {
                ingest_discovery_arm(
                    &mut brain,
                    &mut unicast_discovery_rx_count,
                    recv,
                    &unicast_discovery_read_buf,
                    "ucast",
                );
            }

            // Inbound data packet (42671) → stamp + hand to the runtime mailbox.
            Either::First(Either4::Third(recv)) => {
                ingest_data_arm(id, &mut data_rx_count, recv, &data_read_buf, &inbound);
            }

            // Beacon tick: multicast our token, reverse-peer, age out peers.
            Either::First(Either4::Fourth(_)) => {
                cycle = cycle.wrapping_add(1);
                let now = now_millis().0;
                link_up.store(stack.is_link_up(), Ordering::Relaxed);
                if let Err(e) = discovery_socket
                    .send_to(
                        brain.our_peering_token().as_bytes(),
                        (IpAddress::Ipv6(DISCOVERY_GROUP), DEFAULT_DISCOVERY_PORT),
                    )
                    .await
                {
                    log::warn!("RNS_AUTO beacon send err: {e:?}");
                }
                if cycle % 3 == 0 {
                    let mut targets: HVec<Ipv6Addr, MAX_PEERS> = HVec::new();
                    for addr in brain.known_peer_addresses() {
                        let _ = targets.push(addr);
                    }
                    for addr in targets {
                        let _ = unicast_discovery_socket
                            .send_to(
                                brain.our_peering_token().as_bytes(),
                                (IpAddress::Ipv6(addr), UNICAST_DISCOVERY_PORT),
                            )
                            .await;
                    }
                }
                let pruned = brain.prune_stale_peers(now);
                if pruned > 0 {
                    log::info!("RNS_AUTO pruned {pruned} (peers={})", brain.peer_count());
                }
                peers.store(brain.peer_count() as u16, Ordering::Relaxed);
                log::info!(
                    "RNS_AUTO cyc={cycle} peers={} discovery_rx={discovery_rx_count} unicast_rx={unicast_discovery_rx_count} data_rx={data_rx_count} authfail={}",
                    brain.peer_count(),
                    brain.auth_failures(),
                );
            }
            // Outbound from the manifold: fan out to every peer's data port.
            Either::Second(packet) => {
                let mut targets: HVec<Ipv6Addr, MAX_PEERS> = HVec::new();
                for addr in brain.known_peer_addresses() {
                    let _ = targets.push(addr);
                }
                if targets.is_empty() {
                    log::info!("RNS_AUTO TX {}B dropped — no peers yet", packet.len());
                } else {
                    for addr in &targets {
                        if let Err(e) = data_socket
                            .send_to(&packet, (IpAddress::Ipv6(*addr), DEFAULT_DATA_PORT))
                            .await
                        {
                            log::warn!("RNS_AUTO TX err to {addr}: {e:?}");
                        }
                    }
                    log::info!("RNS_AUTO TX {}B to {} peer(s)", packet.len(), targets.len());
                }
            }
        }
    }
}
