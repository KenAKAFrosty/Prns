use core::net::Ipv6Addr;

use embassy_futures::select::{select, select4, Either, Either4};
use embassy_net::udp::{PacketMetadata, RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_time::{Duration, Instant as EmbassyInstant, Ticker};
use heapless::Vec as HVec;

use super::core::{
    AutoInterfaceProtocol, DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT, DISCOVERY_GROUP,
    HARDWARE_MTU, UNICAST_DISCOVERY_PORT,
};
use crate::engine::InstantMillis;
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{
    ConnectionState, EgressCapability, InboundSink, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, InterfaceWorkerContext, MacAddress,
    MediumKind, TransitCapability,
};

const MAX_PEERS: usize = 8;
const BEACON_INTERVAL_MS: u64 = 1600;

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

pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        medium: MediumKind::Multicast,
        state: ConnectionState::Connected,
    }
}

fn submit_data_arm(
    rx_count: &mut u32,
    recv: Result<(usize, UdpMetadata), RecvError>,
    read_buf: &[u8],
    inbound: &mut impl InboundSink,
) {
    match recv {
        Ok((n, _meta)) => {
            *rx_count = rx_count.wrapping_add(1);
            if n <= HARDWARE_MTU
                && inbound
                    .submit(|buf| {
                        buf[..n].copy_from_slice(&read_buf[..n]);
                        n
                    })
                    .is_err()
            {
                log::warn!("RNS_AUTO inbound ring full, dropped {n}B");
            }
        }
        Err(e) => log::warn!("RNS_AUTO data recv err: {e:?}"),
    }
}

#[allow(clippy::expect_used)]
pub async fn serve<const MAX_BUFFERED_PACKETS: usize>(
    stack: Stack<'static>,
    our_mac_address: MacAddress,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<HARDWARE_MTU, MAX_BUFFERED_PACKETS>>,
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
    let mut out_buf = [0u8; HARDWARE_MTU];
    let mut beacon = Ticker::every(Duration::from_millis(BEACON_INTERVAL_MS));
    let mut cycle: u32 = 0;

    let mut discovery_rx_count: u32 = 0;
    let mut unicast_discovery_rx_count: u32 = 0;
    let mut data_rx_count: u32 = 0;

    loop {
        match select(
            select4(
                discovery_socket.recv_from(&mut discovery_read_buf),
                unicast_discovery_socket.recv_from(&mut unicast_discovery_read_buf),
                data_socket.recv_from(&mut data_read_buf),
                beacon.next(),
            ),
            context.outbound.ready(),
        )
        .await
        {
            Either::First(Either4::First(recv)) => {
                ingest_discovery_arm(
                    &mut brain,
                    &mut discovery_rx_count,
                    recv,
                    &discovery_read_buf,
                    "mcast",
                );
            }

            Either::First(Either4::Second(recv)) => {
                ingest_discovery_arm(
                    &mut brain,
                    &mut unicast_discovery_rx_count,
                    recv,
                    &unicast_discovery_read_buf,
                    "ucast",
                );
            }

            Either::First(Either4::Third(recv)) => {
                submit_data_arm(
                    &mut data_rx_count,
                    recv,
                    &data_read_buf,
                    &mut context.inbound,
                );
            }

            Either::First(Either4::Fourth(_)) => {
                cycle = cycle.wrapping_add(1);
                let now = now_millis().0;
                if let Err(e) = discovery_socket
                    .send_to(
                        brain.our_peering_token().as_bytes(),
                        (IpAddress::Ipv6(DISCOVERY_GROUP), DEFAULT_DISCOVERY_PORT),
                    )
                    .await
                {
                    log::warn!("RNS_AUTO beacon send err: {e:?}");
                }
                if cycle.is_multiple_of(3) {
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
                log::info!(
                    "RNS_AUTO cyc={cycle} peers={} discovery_rx={discovery_rx_count} unicast_rx={unicast_discovery_rx_count} data_rx={data_rx_count} authfail={}",
                    brain.peer_count(),
                    brain.auth_failures(),
                );
            }
            Either::Second(()) => {
                while let Some(len) = context.outbound.try_next_into(&mut out_buf) {
                    let mut targets: HVec<Ipv6Addr, MAX_PEERS> = HVec::new();
                    for addr in brain.known_peer_addresses() {
                        let _ = targets.push(addr);
                    }
                    if targets.is_empty() {
                        log::info!("RNS_AUTO TX {len}B dropped — no peers yet");
                    } else {
                        for addr in &targets {
                            if let Err(e) = data_socket
                                .send_to(
                                    &out_buf[..len],
                                    (IpAddress::Ipv6(*addr), DEFAULT_DATA_PORT),
                                )
                                .await
                            {
                                log::warn!("RNS_AUTO TX err to {addr}: {e:?}");
                            }
                        }
                        log::info!("RNS_AUTO TX {len}B to {} peer(s)", targets.len());
                    }
                }
            }
        }
    }
}
