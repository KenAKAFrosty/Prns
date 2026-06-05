use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket};
use std::time::{Duration, Instant};

use heapless::Vec as HVec;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use super::super::core::{
    descriptor, AutoInterfaceProtocol, DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT, DISCOVERY_GROUP,
    HARDWARE_MTU, UNICAST_DISCOVERY_PORT,
};
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, SelfDrivenInterface,
};
use crate::wire::MTU;

const MAX_PEERS: usize = 8;
const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(250);

type WifiLanContext = InterfaceWorkerContext<StdHostSubstrate<MTU>>;

pub fn wifi_lan_auto_interface(
    id: InterfaceId,
) -> SelfDrivenInterface<impl FnOnce(WifiLanContext)> {
    SelfDrivenInterface::new(descriptor(id), move |context| {
        std::thread::spawn(move || serve(context));
    })
}

fn serve(mut context: WifiLanContext) {
    let Some(nic) = link_local_nic() else {
        eprintln!("WIFI_LAN_AUTO no link-local interface found; worker idle");
        idle_until_stopped(&mut context);
        context.control.report(ControlReport::Stopped);
        return;
    };

    let sockets = match open_sockets(nic.index) {
        Ok(sockets) => sockets,
        Err(e) => {
            eprintln!("WIFI_LAN_AUTO socket setup failed on {}: {e}", nic.name);
            idle_until_stopped(&mut context);
            context.control.report(ControlReport::Stopped);
            return;
        }
    };

    run(&mut context, nic, sockets);
    context.control.report(ControlReport::Stopped);
}

struct Nic {
    name: String,
    link_local: Ipv6Addr,
    index: u32,
}

struct Sockets {
    discovery: UdpSocket,
    unicast_discovery: UdpSocket,
    data: UdpSocket,
}

fn run(context: &mut WifiLanContext, nic: Nic, sockets: Sockets) {
    let Sockets {
        discovery,
        unicast_discovery,
        data,
    } = sockets;

    let mut brain = AutoInterfaceProtocol::<MAX_PEERS>::from_link_local(nic.link_local);
    let token = *brain.our_peering_token().as_bytes();
    eprintln!(
        "WIFI_LAN_AUTO up on {}: link-local {} token {:02x}{:02x}{:02x}{:02x}..",
        nic.name, nic.link_local, token[0], token[1], token[2], token[3],
    );

    let clock = Instant::now();
    let mut last_beacon: Option<Instant> = None;
    let mut beacon_cycle: u32 = 0;
    let mut announced_no_peers = false;

    let mut discovery_buf = [0u8; 64];
    let mut unicast_buf = [0u8; 64];
    let mut data_buf = [0u8; HARDWARE_MTU];

    loop {
        if matches!(context.control.next_command(), Some(ControlCommand::Stop)) {
            return;
        }
        let now_ms = clock.elapsed().as_millis() as u64;

        let mut peers: HVec<Ipv6Addr, MAX_PEERS> = HVec::new();
        for addr in brain.known_peer_addresses() {
            let _ = peers.push(addr);
        }
        context.outbound.drain_each(|packet| {
            if peers.is_empty() {
                if !announced_no_peers {
                    eprintln!("WIFI_LAN_AUTO TX dropped — no peers yet");
                    announced_no_peers = true;
                }
                return;
            }
            for addr in &peers {
                let _ = data.send_to(packet.bytes, scoped(*addr, DEFAULT_DATA_PORT, nic.index));
            }
        });
        if !peers.is_empty() {
            announced_no_peers = false;
        }

        let mut did_work = false;
        did_work |= ingest_discovery(&discovery, &mut discovery_buf, &mut brain, now_ms, "mcast");
        did_work |= ingest_discovery(
            &unicast_discovery,
            &mut unicast_buf,
            &mut brain,
            now_ms,
            "ucast",
        );
        did_work |= ingest_data(&data, &mut data_buf, context);

        if last_beacon.is_none_or(|t| t.elapsed() >= BEACON_INTERVAL) {
            last_beacon = Some(Instant::now());
            beacon_cycle = beacon_cycle.wrapping_add(1);
            let _ = discovery.send_to(
                &token,
                scoped(DISCOVERY_GROUP, DEFAULT_DISCOVERY_PORT, nic.index),
            );
            if beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY) {
                let mut targets: HVec<Ipv6Addr, MAX_PEERS> = HVec::new();
                for addr in brain.known_peer_addresses() {
                    let _ = targets.push(addr);
                }
                for addr in &targets {
                    let _ = unicast_discovery
                        .send_to(&token, scoped(*addr, UNICAST_DISCOVERY_PORT, nic.index));
                }
            }
            let pruned = brain.prune_stale_peers(now_ms);
            if pruned > 0 {
                eprintln!("WIFI_LAN_AUTO pruned {pruned} (peers={})", brain.peer_count());
            }
        }

        if !did_work {
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

fn ingest_discovery(
    socket: &UdpSocket,
    buf: &mut [u8],
    brain: &mut AutoInterfaceProtocol<MAX_PEERS>,
    now_ms: u64,
    via: &str,
) -> bool {
    match socket.recv_from(buf) {
        Ok((n, src)) => {
            if let Some(addr) = ipv6_of(src) {
                let before = brain.peer_count();
                brain.ingest_discovery_datagram(addr, &buf[..n], now_ms);
                if brain.peer_count() > before {
                    eprintln!(
                        "WIFI_LAN_AUTO PEER+ {addr} via {via} (peers={})",
                        brain.peer_count()
                    );
                }
            }
            true
        }
        Err(e) if would_block(&e) => false,
        Err(e) => {
            eprintln!("WIFI_LAN_AUTO {via} recv err: {e}");
            false
        }
    }
}

fn ingest_data(socket: &UdpSocket, buf: &mut [u8], context: &mut WifiLanContext) -> bool {
    match socket.recv_from(buf) {
        Ok((n, _src)) => {
            if n <= MTU {
                let _ = context.inbound.submit(|slot| {
                    slot[..n].copy_from_slice(&buf[..n]);
                    n
                });
            }
            true
        }
        Err(e) if would_block(&e) => false,
        Err(e) => {
            eprintln!("WIFI_LAN_AUTO data recv err: {e}");
            false
        }
    }
}

fn link_local_nic() -> Option<Nic> {
    for iface in if_addrs::get_if_addrs().ok()? {
        if iface.is_loopback() || !matches!(iface.addr, if_addrs::IfAddr::V6(_)) {
            continue;
        }
        let Some(index) = iface.index else {
            continue;
        };
        if let Some(link_local) = link_local_for_scope(index) {
            return Some(Nic {
                name: iface.name,
                link_local,
                index,
            });
        }
    }
    None
}

fn link_local_for_scope(index: u32) -> Option<Ipv6Addr> {
    let probe = UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)).ok()?;
    let target = SocketAddrV6::new(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), 9999, 0, index);
    probe.connect(SocketAddr::V6(target)).ok()?;
    let SocketAddr::V6(local) = probe.local_addr().ok()? else {
        return None;
    };
    let link_local = *local.ip();
    ((link_local.segments()[0] & 0xffc0) == 0xfe80).then_some(link_local)
}

fn open_sockets(index: u32) -> io::Result<Sockets> {
    Ok(Sockets {
        discovery: discovery_socket(index)?,
        unicast_discovery: unicast_socket(UNICAST_DISCOVERY_PORT)?,
        data: unicast_socket(DEFAULT_DATA_PORT)?,
    })
}

fn discovery_socket(index: u32) -> io::Result<UdpSocket> {
    let socket = reusable_udp_v6(DEFAULT_DISCOVERY_PORT)?;
    socket.set_multicast_if_v6(index)?;
    socket.join_multicast_v6(&DISCOVERY_GROUP, index)?;
    Ok(socket.into())
}

fn unicast_socket(port: u16) -> io::Result<UdpSocket> {
    Ok(reusable_udp_v6(port)?.into())
}

fn reusable_udp_v6(port: u16) -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    let bind = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0));
    socket.bind(&SockAddr::from(bind))?;
    Ok(socket)
}

fn idle_until_stopped(context: &mut WifiLanContext) {
    loop {
        if matches!(context.control.next_command(), Some(ControlCommand::Stop)) {
            return;
        }
        std::thread::sleep(IDLE_POLL_INTERVAL);
    }
}

fn scoped(addr: Ipv6Addr, port: u16, index: u32) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(addr, port, 0, index))
}

fn ipv6_of(src: SocketAddr) -> Option<Ipv6Addr> {
    match src {
        SocketAddr::V6(v6) => Some(*v6.ip()),
        SocketAddr::V4(_) => None,
    }
}

fn would_block(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
