//! The embassy-net worker shell for the RNS AutoInterface — the ESP32 family
//! (S3, C6) and any embassy-net host.
//!
//!  The host supplies the WiFi/IP stack, the
//! channels (`'static`), the interface id, and an entropy source; everything
//! about *how this talks to the world* stays in here.

use core::net::Ipv6Addr;

use embassy_futures::select::{select, select4, Either, Either4};
use embassy_net::udp::{PacketMetadata, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant as EmbassyInstant, Ticker};
use heapless::Vec as HVec;

use super::core::{
    AutoInterfaceProtocol, DATA_PORT, DISCOVERY_GROUP, DISCOVERY_PORT, HW_MTU,
    UNICAST_DISCOVERY_PORT,
};
use crate::engine::InstantMillis;
use crate::interfaces::{
    Capabilities, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceMode, InterfaceStats,
    InterfaceWorker, MediumKind, QueueFull,
};
use crate::runtime::manifold::impls::embassy::{InboundSender, InboxEntry};

pub const OUTBOX_DEPTH: usize = 4;
const CHANNEL_PACKET_LEN_CAP: usize = HW_MTU;
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
/// [`HW_MTU`].
pub type OutboundChannel = Channel<CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundSender = Sender<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundReceiver = Receiver<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;

pub struct EmbassyAutoInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
}

impl EmbassyAutoInterface {
    /// Build the handle for interface `id`, sending to the worker over `outbound`.
    pub fn new(id: InterfaceId, outbound: OutboundSender) -> Self {
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
        }
    }
}

impl InterfaceWorker for EmbassyAutoInterface {
    // RNS pins the AutoInterface MTU at 1196 bytes; the inbound mailbox sizes to
    // it so a stamped packet always fits.
    const PACKET_BUFFER_SIZE: usize = HW_MTU;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn health(&self) -> InterfaceStats {
        // Slice-1 stub: the interface is up once registered. Live counters
        // (peers/rx/tx) come from the worker's own log for now; wire a shared
        // snapshot here when health drives the OLED/UI.
        InterfaceStats {
            online: true,
            ..InterfaceStats::default()
        }
    }

    //REVIEW seems like the interfaceworker should still get the typed EgressDirective or some other typed data structure right? I'll need to check, this is worth an iterative discussion about
    fn submit(&mut self, packet: &[u8]) -> Result<(), QueueFull> {
        let buf = PacketBuf::from_slice(packet).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
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

/// The worker's own task/mini-program: own the three UDP sockets, drive the discovery brain,
/// stamp inbound data into `inbound`, and fan `outbound` writes to every peer.
/// Runs forever. `stack` must already be up (host brought up WiFi + IP).
pub async fn run(
    stack: Stack<'static>,
    id: InterfaceId,
    our_mac_address: [u8; 6], //REVIEW yeah again probably newtype here?
    inbound: InboundSender<HW_MTU>,
    outbound: OutboundReceiver,
) {
    let mut brain = AutoInterfaceProtocol::<MAX_PEERS>::new(our_mac_address);
    log::info!(
        "RNS_AUTO up: link-local {} token {:02x}{:02x}{:02x}{:02x}..",
        brain.our_link_local(),
        brain.our_peering_token()[0],
        brain.our_peering_token()[1],
        brain.our_peering_token()[2],
        brain.our_peering_token()[3],
    );

    match stack.join_multicast_group(IpAddress::Ipv6(DISCOVERY_GROUP)) {
        Ok(()) => log::info!("RNS_AUTO joined {DISCOVERY_GROUP}"),
        Err(e) => log::warn!("RNS_AUTO mcast join failed: {e:?}"),
    }

    //REVEIEW better names for all these. "disc" and "udisc" are not very helpful at all.
    let mut disc_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut disc_rx = [0u8; 512];
    let mut disc_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut disc_tx = [0u8; 512];
    let mut disc = UdpSocket::new(
        stack,
        &mut disc_rx_meta,
        &mut disc_rx,
        &mut disc_tx_meta,
        &mut disc_tx,
    );
    disc.bind(DISCOVERY_PORT).expect("bind discovery port");

    let mut udisc_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut udisc_rx = [0u8; 512];
    let mut udisc_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut udisc_tx = [0u8; 512];
    let mut udisc = UdpSocket::new(
        stack,
        &mut udisc_rx_meta,
        &mut udisc_rx,
        &mut udisc_tx_meta,
        &mut udisc_tx,
    );
    udisc
        .bind(UNICAST_DISCOVERY_PORT)
        .expect("bind unicast discovery port");

    let mut data_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_rx = [0u8; 1280];
    let mut data_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_tx = [0u8; 1280];
    let mut data = UdpSocket::new(
        stack,
        &mut data_rx_meta,
        &mut data_rx,
        &mut data_tx_meta,
        &mut data_tx,
    );
    data.bind(DATA_PORT).expect("bind data port");

    //REVIEW the names! the names! my kingdom for a legible name!
    let mut drx = [0u8; HW_MTU];
    let mut rx = [0u8; 64];
    let mut urx = [0u8; 64];
    let mut beacon = Ticker::every(Duration::from_millis(BEACON_INTERVAL_MS));
    let mut cycle: u32 = 0;

    // RX activity counters — so a status line each tick shows whether we keep
    // receiving (vs. discover-once-then-go-deaf).

    // REVIEW again, better names here would not only help but make the above comment unnecessary
    let mut mcast_rx: u32 = 0;
    let mut ucast_rx: u32 = 0;
    let mut data_rx: u32 = 0;

    loop {
        match select(
            select4(
                disc.recv_from(&mut rx),
                udisc.recv_from(&mut urx),
                data.recv_from(&mut drx),
                beacon.next(),
            ),
            outbound.receive(),
        )
        .await
        {
            //REVIEW helper functions for each arm will keep the matching branch site easier to understand and I think is very very worth doing now
            // Multicast discovery beacon (29716).
            Either::First(Either4::First(Ok((n, meta)))) => {
                mcast_rx = mcast_rx.wrapping_add(1);
                if let Some(src) = ipv6_src(&meta) {
                    let before = brain.peer_count();
                    brain.ingest_discovery_datagram(src, &rx[..n], now_millis().0);
                    if brain.peer_count() > before {
                        log::info!(
                            "RNS_AUTO PEER+ {src} via mcast (peers={})",
                            brain.peer_count()
                        );
                    }
                }
            }

            Either::First(Either4::First(Err(e))) => log::warn!("RNS_AUTO disc recv err: {e:?}"),

            // Unicast reverse-peering beacon (29717).
            Either::First(Either4::Second(Ok((n, meta)))) => {
                ucast_rx = ucast_rx.wrapping_add(1);
                if let Some(src) = ipv6_src(&meta) {
                    let before = brain.peer_count();
                    brain.ingest_discovery_datagram(src, &urx[..n], now_millis().0);
                    if brain.peer_count() > before {
                        log::info!(
                            "RNS_AUTO PEER+ {src} via ucast (peers={})",
                            brain.peer_count()
                        );
                    }
                }
            }
            Either::First(Either4::Second(Err(e))) => log::warn!("RNS_AUTO udisc recv err: {e:?}"),

            // Inbound data packet (42671) → stamp + hand to the runtime mailbox.
            Either::First(Either4::Third(Ok((n, _meta)))) => {
                data_rx = data_rx.wrapping_add(1);
                if let Ok(bytes) = PacketBuf::from_slice(&drx[..n]) {
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

            Either::First(Either4::Third(Err(e))) => log::warn!("RNS_AUTO data recv err: {e:?}"),

            // Beacon tick: multicast our token, reverse-peer, age out peers.
            Either::First(Either4::Fourth(_)) => {
                cycle = cycle.wrapping_add(1);
                let now = now_millis().0;
                if let Err(e) = disc
                    .send_to(
                        brain.our_peering_token(),
                        (IpAddress::Ipv6(DISCOVERY_GROUP), DISCOVERY_PORT),
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
                        let _ = udisc
                            .send_to(
                                brain.our_peering_token(),
                                (IpAddress::Ipv6(addr), UNICAST_DISCOVERY_PORT),
                            )
                            .await;
                    }
                }
                let pruned = brain.prune_stale_peers(now);
                if pruned > 0 {
                    log::info!("RNS_AUTO pruned {pruned} (peers={})", brain.peer_count());
                }
                log::info!(
                    "RNS_AUTO cyc={cycle} peers={} mcast_rx={mcast_rx} ucast_rx={ucast_rx} data_rx={data_rx} authfail={}",
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
                        if let Err(e) = data
                            .send_to(&packet, (IpAddress::Ipv6(*addr), DATA_PORT))
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
